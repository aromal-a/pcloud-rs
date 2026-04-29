//! `RuntimeShell`: the long-lived in-process state that backends mutate.
//! Holds protocol clients, the filesystem shell, store handles, crypto
//! state, sync roots, pending transfers, and the auth vault. Mutations
//! go through typed backend APIs; this module defines the aggregate.
//!
//! **Platform banner:** sync/mount runtime is Linux-gated; portable
//! bookkeeping (auth, transfers, public links) works everywhere the
//! workspace builds.

// **PLATFORM:** Linux
// **GATING:** #[cfg(target_os = "linux")].

use std::path::{Path, PathBuf};

// H14 PR4 — integrity sweeper service. Declared here (rather than in
// `lib.rs`) so PR4 can ship the new file without touching the crate-
// root module list. The `#[path]` attribute pins the file location at
// `crates/pcloud-daemon/src/integrity_sweeper_service.rs`. Bootstrap
// wiring is tracked under bd-1du.4.6.1 — see TODO in
// `RuntimeShell::bootstrap_integrity_sweeper` below.
#[path = "integrity_sweeper_service.rs"]
pub mod integrity_sweeper_service;

use pcloud_auth::{AuthCommand, AuthFlowError, SessionManager};
use pcloud_cache::CacheShell;
use pcloud_crypto::CryptoShell;
use pcloud_engine::EngineShell;
use pcloud_fs::FilesystemShell;
use pcloud_ipc::methods::CryptoBackendIpc;
use pcloud_ipc::{Request, Response, ResponseStatus};
use pcloud_model::ids::UserId;
use pcloud_model::public_links::PublicLinkUploadPolicy;
use pcloud_model::shares::SharePermissions;
use pcloud_model::sync::SyncState;
use pcloud_observability::ObservabilityShell;
#[cfg(feature = "metrics")]
use pcloud_observability::metrics::{AuthResult, CryptoLockState, TransferDirection};
use pcloud_p2p::P2pShell;
use pcloud_secret::{ExposeSecret, secret_string::SecretString};
use pcloud_store::{
    StoreProfile, append_audit_event, integrity::IntegrityStatus, persist_profile,
    repositories::account::AccountRecord, repositories::sync_graph::SyncRootRecord,
};

use crate::account_backend::AccountRuntime;
use crate::auth_backend::{AuthBackendError, AuthRuntime};
use crate::auth_vault::{AuthVaultError, clear_token, load_token, store_token};
use crate::backup_backend::BackupRuntime;
use crate::crypto_backend::CryptoRuntime;
use crate::folder_backend::{FolderBackendError, FolderRuntime};
use crate::mount_runtime::MountControl;
use crate::notifications_backend::{NotificationsBackendError, NotificationsRuntime};
use crate::public_link_backend::{PublicLinkBackendError, PublicLinkRuntime};
use crate::shares_backend::{SharesBackendError, SharesRuntime};
use crate::sync_backend::SyncRuntime;
use crate::transfer_backend::TransferRuntime;
use crate::transport_factory::TransportFactory;

/// Runtime control flags shared across the daemon's dispatch paths.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RuntimeControlState {
    /// Set to `true` once a shutdown has been requested (via IPC
    /// `Method::Shutdown` or a terminating signal). The serve loop and
    /// health endpoints consume this flag.
    pub shutdown_requested: bool,
}

/// Holds username and password material captured during an interactive
/// login while a TFA challenge is still pending.
///
/// Lives only for the duration of the pending challenge; dropped (and
/// zeroized) as soon as the user completes or cancels TFA.
#[derive(Debug, PartialEq, Eq)]
pub struct PendingPasswordAuth {
    /// Account username attached to the pending credential.
    pub username: String,
    /// Account password; held in a zeroising [`SecretString`] so Drop
    /// scrubs the buffer if the TFA challenge is abandoned.
    pub password: SecretString,
}

impl Clone for PendingPasswordAuth {
    // Audit-visible duplication: `SecretString` does not derive `Clone`
    // (audit M3). Delegate to `clone_secret` so every duplication of the
    // password buffer is conspicuous in code review.
    fn clone(&self) -> Self {
        Self {
            username: self.username.clone(),
            password: self.password.clone_secret(),
        }
    }
}

/// Composition root for the running daemon.
///
/// Owns every per-subsystem runtime, the session manager, the store,
/// the IPC ownership metadata, and the mount controller. IPC dispatch
/// (`handle_request`) borrows this shell mutably for the duration of a
/// single request.
///
/// # Lifetime
///
/// Constructed exactly once by [`crate::bootstrap::bootstrap_shell`]
/// (or [`crate::bootstrap::bootstrap_with_config`]) and dropped when
/// the daemon exits. The drop order scrubs zeroising secret buffers
/// (`SecretString` / `SecretBytes`) in the session manager, crypto
/// shell, and pending TFA state before the store handle is released.
///
/// # Thread-safety
///
/// `RuntimeShell` is intentionally `!Sync` and is only mutated from a
/// single IPC dispatch thread. Sub-shells that need cross-thread
/// visibility (metrics `MetricsBridge`, mount drain hooks, SLO
/// registry, refresh guard) expose their own `Arc`/atomic-backed
/// handles rather than sharing the shell directly. This keeps the
/// dispatch path lock-free and makes the panic-guard in
/// [`RuntimeShell::handle_request`] sound (no poisoned mutexes on
/// unwind).
#[derive(Debug)]
pub struct RuntimeShell {
    /// Validated configuration profile that drove bootstrap.
    pub config: pcloud_config::ConfigProfile,
    /// SQLite-backed store handle (schema migrated, integrity checked).
    pub store: StoreProfile,
    /// Result of the last store integrity check.
    pub integrity: IntegrityStatus,
    /// Authoritative in-memory session state machine.
    pub auth: SessionManager,
    /// Auth protocol runtime (login, TFA, token refresh).
    pub auth_runtime: AuthRuntime,
    /// Account maintenance runtime (password change, email, promos).
    pub account_runtime: AccountRuntime,
    /// Backup-device lifecycle runtime.
    pub backup_runtime: BackupRuntime,
    /// Crypto (Crypto Folder) protocol runtime.
    pub crypto_runtime: CryptoRuntime,
    /// Remote-folder operations runtime (create/stat/list).
    pub folder_runtime: FolderRuntime,
    /// Notifications fetch/mark-read runtime.
    pub notifications_runtime: NotificationsRuntime,
    /// Public-link lifecycle runtime.
    pub public_link_runtime: PublicLinkRuntime,
    /// Shares/business/team runtime.
    pub shares_runtime: SharesRuntime,
    /// Sync-root lifecycle runtime.
    pub sync_runtime: SyncRuntime,
    /// Upload/download transfer runtime.
    pub transfer_runtime: TransferRuntime,
    /// Sync engine scheduler shell.
    pub engine: EngineShell,
    /// Local cache shell (page/metadata caches).
    pub cache: CacheShell,
    /// Filesystem shell (mounted-drive state used by IPC surfaces).
    pub filesystem: FilesystemShell,
    /// Crypto shell (unlock state, key material cache).
    pub crypto: CryptoShell,
    /// Observability shell (metric families, SLO bridge).
    pub observability: ObservabilityShell,
    /// P2P/LAN discovery shell.
    pub p2p: P2pShell,
    /// Runtime control flags (shutdown request, etc.).
    pub control: RuntimeControlState,
    /// UID that owns the IPC socket; used to enforce peer-UID checks.
    pub ipc_owner_uid: Option<u32>,
    /// Captured password credentials waiting for a TFA completion.
    pub pending_password_auth: Option<PendingPasswordAuth>,
    /// bd-1du.4.e sub-task 2: owns the active FUSE mount (if any) and the
    /// drain-on-unmount hook.
    pub mount_control: MountControl,
    /// Resilience transport factory. In production the factory wraps
    /// outbound transports in `ResilientTransport`; in development and
    /// test environments it reports a bare/direct-dispatch decision to
    /// preserve existing test determinism.
    pub transport_factory: TransportFactory,
    /// Session-supervisor (sub-task 3): classifies session timing,
    /// runs single-flight proactive refresh, and handles idle logout.
    /// The supervisor owns an `Arc<RefreshGuard>` so status reporting
    /// (`Method::SessionStatus`) can observe in-flight refreshes
    /// without contending with the refresh path.
    pub session_supervisor: crate::session_lifecycle::SessionSupervisor,
    /// H14 PR4 — background integrity sweeper. Defaults to a disabled
    /// shell so the runtime stays a no-op for operators who have not
    /// opted into `[features.integrity_sweeper] enabled = true`. The
    /// real worker is spawned by
    /// [`RuntimeShell::bootstrap_integrity_sweeper`].
    pub integrity_sweeper: integrity_sweeper_service::IntegritySweeperShell,
    /// Scheduled audit-chain verifier (I04 follow-up). Defaults to a
    /// disabled shell; when `[features.audit_verifier] enabled = true`
    /// (the default) the cron scheduler runs at 03:00 daily. IPC
    /// `Method::GetAuditVerifierStatus` reads the shell snapshot.
    pub audit_verifier: crate::audit_verifier_service::AuditVerifierShell,
    /// Data-residency region cache used by the three enforcement call
    /// sites (`sync_root_add`, `upload_create`, and public-link create).
    /// Empty by default; entries are populated opportunistically by
    /// [`RuntimeShell::check_residency`] and expire per
    /// [`pcloud_backends::residency::REGION_CACHE_TTL`] (1h).
    pub residency_cache: pcloud_backends::residency::RegionCache,
    /// Tier-2 HA posture (see `docs/enterprise/ha.md` §4.2). Populated
    /// by `bootstrap_with_config` when `[ha].enabled = true`. Defaults
    /// to [`crate::ha_lease::HaRuntime::disabled`] so existing
    /// callers / tests keep the single-instance behaviour with no
    /// changes.
    pub ha: crate::ha_lease::HaRuntime,
    /// Per-peer IPC rate limiter. Built from
    /// `config.rate_limit` at bootstrap; consulted by
    /// `dispatch::handle_request` before every backend call.
    /// Over-budget callers receive `ResponseStatus::Conflict`
    /// without the backend being invoked. Distinct peer uids maintain
    /// independent token-bucket state so a single chatty client cannot
    /// starve other authorized peers. See
    /// [`crate::rate_limit::PerPeerRateLimiter`].
    pub rate_limiter: crate::rate_limit::PerPeerRateLimiter,
    /// Operator-visible upload session registry (pause / resume /
    /// cancel / list). Populated by `Request::UploadCreate`; mutated
    /// by the four companion IPC variants. In-memory only — the
    /// crash-safe wire-protocol resume state is still the SQLite
    /// `upload_resume_state` table + NDJSON journal.
    pub upload_sessions: pcloud_backends::upload_sessions::SessionRegistry,
    /// Shared state for the background sync loop. Populated by
    /// bootstrap when `config.sync_loop.enabled = true`. The IPC
    /// thread reads status via `shared.current_status()` and wakes
    /// the loop via `shared.wake()`. `None` when the loop is disabled
    /// or not yet initialized.
    pub sync_loop_shared: Option<std::sync::Arc<crate::sync_loop::SyncLoopShared>>,
    /// Path to the on-disk config file that was loaded at bootstrap.
    /// `None` when the daemon was bootstrapped from in-memory defaults
    /// (tests, embedded SDK). Used by SIGHUP hot-reload to re-read the
    /// file without the daemon needing to know the path at every call
    /// site.
    pub config_path: Option<std::path::PathBuf>,
    /// Per-request peer PID stash (audit context).
    ///
    /// Populated by `dispatch::dispatch_with_peer` at the top of every
    /// IPC dispatch (from `SO_PEERCRED` / `getpeereid` / named-pipe
    /// client id resolved by pcloud-ipc) and cleared at the end. `None`
    /// for non-IPC callers (embedders, tests). Downstream audit sites
    /// that emit privileged-action events can read this alongside
    /// `ipc_owner_uid` to include PID in the audit trail without
    /// re-threading the value through every handler signature.
    ///
    /// Fixes ncx.54 (P3-E1 dispatch_with_drain_gate was dropping
    /// peer_pid before dispatch, losing audit context downstream).
    pub current_peer_pid: Option<u32>,
}

impl RuntimeShell {
    /// Render a human-readable one-line summary of the runtime state,
    /// used by IPC `GetStatus` and by daemon startup logging.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "daemon runtime ready: env={:?}, schema_v={}, integrity={:?}, {}, {}, {}, {}, {}, {}",
            self.config.environment,
            self.store.schema_version,
            self.integrity,
            self.engine.summary(),
            self.cache.summary(),
            self.filesystem.summary(),
            self.crypto.summary(),
            self.observability.summary(),
            self.p2p.summary()
        )
    }

    /// Apply hot-reloadable fields from a freshly loaded config profile.
    ///
    /// Called by the serve loop after SIGHUP triggers a config re-read.
    /// Only fields classified as hot-reloadable in
    /// [`crate::config_reload`] are applied; restart-only fields (auth
    /// vault path, IPC socket path, crypto master key) are ignored.
    pub fn apply_hot_reload(&mut self, new: pcloud_config::ConfigProfile) {
        // Observability flags
        self.config.observability = new.observability;

        // Rate-limit budgets — rebuild the per-session limiter from the
        // new policy so the next IPC request picks up the change.
        self.config.rate_limit = new.rate_limit.clone();
        self.rate_limiter.apply_policy(&new.rate_limit);

        // Integrity sweeper schedule
        self.config.features.integrity_sweeper = new.features.integrity_sweeper;

        // Sync poll interval
        self.config.sync_loop = new.sync_loop;

        // Data-residency allow-list
        self.config.data_residency = new.data_residency;
    }

    /// Top-level IPC dispatch entry point.
    ///
    /// Wraps the internal dispatch routine in a panic guard and, when
    /// the `metrics` feature is active, records per-method latency and
    /// a panic counter. Returns an internal-error response rather than
    /// propagating a panic.
    ///
    /// # SLO instrumentation
    ///
    /// The SLO registry on `self.observability.slo` is always-present
    /// (not feature-gated) so this path unconditionally times the
    /// dispatch and feeds two canonical SLI samples:
    ///   - `ipc.request.latency.p99` via
    ///     [`pcloud_observability::slo::Slo::observe_ipc_latency`]
    ///   - `ipc.request.error_rate` via
    ///     [`pcloud_observability::slo::Slo::observe_ipc_outcome`]
    ///     (counting non-`Ok` responses and caught panics as errors).
    pub fn handle_request(&mut self, request: Request) -> Response {
        #[cfg(feature = "metrics")]
        let method_label = method_label(&request);
        #[cfg(feature = "metrics")]
        let started = std::time::Instant::now();
        // Always-on SLO timer. Independent of the `metrics` feature so
        // `Method::GetSlo` returns real samples in every build profile.
        let slo_started = std::time::Instant::now();

        // Panic guard is unconditional (production build must contain the
        // catch_unwind boundary so a buggy dispatch branch cannot take the
        // whole daemon down). The metrics-counter increment stays
        // feature-gated because the counter family only exists when the
        // `metrics` feature is compiled in.
        let response = {
            use std::panic::{AssertUnwindSafe, catch_unwind};
            match catch_unwind(AssertUnwindSafe(|| self.handle_request_dispatch(request))) {
                Ok(resp) => resp,
                Err(_panic) => {
                    #[cfg(feature = "metrics")]
                    self.observability.families.incr_panic();
                    // Notify the dispatch span (if `tracing-otlp` is on) so
                    // it can record `status_code = "panic"` and emit an
                    // error event before being closed. Compiles to a no-op
                    // when the feature is off.
                    crate::dispatch::note_dispatch_panic();
                    Response {
                        status: ResponseStatus::InternalError,
                        message: "internal daemon panic; request aborted".to_owned(),
                    }
                }
            }
        };

        #[cfg(feature = "metrics")]
        {
            let latency = started.elapsed().as_secs_f64();
            self.observability.families.observe_request(
                method_label,
                status_label(&response.status),
                latency,
            );
            // Fold any process-wide panic hook signals into the gauge so
            // that a panic in a background thread (outside handle_request)
            // still becomes visible in the next Prometheus scrape.
            self.metric_refresh_panic_count();
        }

        // SLO wiring (I15 hot-path call site #1 + #2).
        //
        // Record the canonical IPC SLIs on every IPC round-trip,
        // independent of the `metrics` feature flag. `observe_ipc_outcome`
        // only increments the error counter when `error == true`, so
        // success calls cost a single atomic fence on the latency path.
        let slo_latency = slo_started.elapsed().as_secs_f64();
        self.observability.slo.observe_ipc_latency(slo_latency);
        self.observability
            .slo
            .observe_ipc_outcome(!matches!(response.status, ResponseStatus::Ok));
        response
    }

    fn handle_request_dispatch(&mut self, request: Request) -> Response {
        // Tier-2 HA: a passive daemon rejects every request with
        // `Unavailable` + a message naming the primary. We allow a
        // narrow allow-list through so observability / probe surfaces
        // keep working (HaStatus, GetHealth, Health, and Shutdown — so
        // operators can still stop a passive daemon cleanly).
        if self.ha.is_passive() {
            let allowed = matches!(
                &request,
                Request::Plain {
                    method: pcloud_ipc::Method::HaStatus
                        | pcloud_ipc::Method::GetHealth
                        | pcloud_ipc::Method::Health
                        | pcloud_ipc::Method::Shutdown,
                }
            );
            if !allowed {
                let payload = self.ha.status_payload();
                return Response {
                    status: ResponseStatus::Unavailable,
                    message: payload.passive_rejection_message(),
                };
            }
        }
        match request {
            Request::Plain { method } => match method {
                pcloud_ipc::Method::GetStatus => Response {
                    status: ResponseStatus::Ok,
                    message: format!(
                        "status: auth={:?}, last_auth_error={:?}, authsave_enabled={}, sync={:?}, crypto={:?}, shutdown_requested={}, {}",
                        self.auth.snapshot().state,
                        self.auth.snapshot().last_auth_error,
                        self.config.features.durable_auth_tokens_enabled,
                        self.engine.sync_state,
                        self.crypto.unlock_state,
                        self.control.shutdown_requested,
                        self.engine.summary()
                    ),
                },
                pcloud_ipc::Method::Health => Response {
                    status: ResponseStatus::Ok,
                    message: format!(
                        "health (enterprise): integrity={:?}, sync={:?}, crypto={:?}, shutdown={}, {}",
                        self.integrity,
                        self.engine.sync_state,
                        self.crypto.unlock_state,
                        self.control.shutdown_requested,
                        self.engine.summary()
                    ),
                },
                pcloud_ipc::Method::GetHealth => Response {
                    status: ResponseStatus::Ok,
                    message: format!(
                        "health: integrity={:?}, sync={:?}, crypto={:?}, {}",
                        self.integrity,
                        self.engine.sync_state,
                        self.crypto.unlock_state,
                        self.engine.summary()
                    ),
                },
                pcloud_ipc::Method::GetPending => self.pending_transfers(),
                pcloud_ipc::Method::GetSyncRoots => self.list_sync_roots(),
                pcloud_ipc::Method::ListPublicLinks => self.list_public_links(),
                pcloud_ipc::Method::ListUploadLinks => self.list_upload_links(),
                pcloud_ipc::Method::GetUserInfo => self.fetch_userinfo(),
                pcloud_ipc::Method::PauseSync => self.pause_sync(),
                pcloud_ipc::Method::ResumeSync => self.resume_sync(),
                pcloud_ipc::Method::LoginBegin => match self.auth.apply(AuthCommand::BeginLogin) {
                    Ok(event) => Response {
                        status: ResponseStatus::Ok,
                        message: format!("auth event: {:?}", event),
                    },
                    Err(err) => Response {
                        status: ResponseStatus::Conflict,
                        message: err.to_string(),
                    },
                },
                pcloud_ipc::Method::Logout => self.logout(),
                pcloud_ipc::Method::SendTwoFactorSms => self.send_two_factor_sms(),
                pcloud_ipc::Method::SendTwoFactorNotification => {
                    self.send_two_factor_notification()
                }
                pcloud_ipc::Method::UnlockCrypto => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "use structured CryptoUnlock / CryptoSetup request variant".to_owned(),
                },
                pcloud_ipc::Method::SetAuthPersistence => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "use structured auth persistence request variant".to_owned(),
                },
                pcloud_ipc::Method::GetCryptoStatus => self.crypto_status(),
                pcloud_ipc::Method::CryptoReset => self.crypto_reset(),
                pcloud_ipc::Method::LockCrypto => self.lock_crypto(),
                pcloud_ipc::Method::GetCryptoPrivKeyFlags => self.crypto_priv_key_flags(),
                pcloud_ipc::Method::SendCryptoChangeUserPrivate => {
                    self.send_crypto_change_user_private()
                }
                pcloud_ipc::Method::Shutdown => self.request_shutdown(),
                pcloud_ipc::Method::ListIncomingShares => self.list_shares(true),
                pcloud_ipc::Method::ListOutgoingShares => self.list_shares(false),
                pcloud_ipc::Method::ListIncomingShareRequests => self.list_share_requests(true),
                pcloud_ipc::Method::ListOutgoingShareRequests => self.list_share_requests(false),
                pcloud_ipc::Method::ListContacts => self.list_contacts(),
                pcloud_ipc::Method::ListMyTeams => self.list_my_teams(),
                pcloud_ipc::Method::ListNotifications => self.list_notifications(),
                pcloud_ipc::Method::SessionStatus => self.session_status(),
                // H14 PR4 — integrity sweeper status. See bd-1du.4.6.1.
                pcloud_ipc::Method::IntegrityStatus => self.integrity_status(),
                // Canonical Service-Level Objectives report. See
                // `crates/pcloud-observability/src/slo.rs` for the
                // canonical SLO set and thresholds.
                pcloud_ipc::Method::GetSlo => self.get_slo(),
                // Tier-2 HA status. See `docs/enterprise/ha.md` §4.2 and
                // `crates/pcloud-daemon/src/ha_lease.rs`.
                pcloud_ipc::Method::HaStatus => self.ha_status(),
                // Graceful-drain status probe. Always safe to call;
                // admitted by the drain gate so operators can poll
                // progress during shutdown. See
                // `crates/pcloud-daemon/src/serve.rs` for the gate
                // and `pcloud-ipc::DrainStatusPayload` for the wire
                // shape.
                pcloud_ipc::Method::DrainStatus => crate::runtime::drain_status_response(),
                // Scheduled audit-chain verifier status. Always safe;
                // returns `enabled = false` when disabled.
                pcloud_ipc::Method::GetAuditVerifierStatus => self.audit_verifier_status(),
                // Background sync loop status. Always safe to call.
                pcloud_ipc::Method::GetSyncStatus => self.sync_loop_status(),
                pcloud_ipc::Method::ListConflicts => self.list_conflicts(),
                pcloud_ipc::Method::StatPath => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "use structured StatPath request variant".to_owned(),
                },
                pcloud_ipc::Method::GetApiServers => self.get_api_servers(),
                pcloud_ipc::Method::GetPromo => self.get_promo(),
                pcloud_ipc::Method::GetCryptoHint => self.get_crypto_hint(),
                pcloud_ipc::Method::VerifyEmail => self.verify_email(),
                pcloud_ipc::Method::SubmitPassword | pcloud_ipc::Method::SubmitTwoFactorCode => {
                    Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "use structured secret-bearing request variant".to_owned(),
                    }
                }
                // `Method` is `#[non_exhaustive]`: newly added variants in
                // future pcloud-ipc versions must be rejected explicitly
                // rather than silently handled.
                _ => Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "unsupported ipc method (newer client than daemon?)".to_owned(),
                },
            },
            Request::PasswordSubmission { username, value } => {
                self.pending_password_auth = Some(PendingPasswordAuth {
                    username: username.clone(),
                    password: SecretString::new(value.clone()),
                });
                match self.auth_runtime.login_with_password(
                    &mut self.auth,
                    username,
                    SecretString::new(value),
                ) {
                    Ok(event) => self.auth_response(event),
                    Err(err) => {
                        self.pending_password_auth = None;
                        map_auth_flow_error(err)
                    }
                }
            }
            Request::AuthTokenSubmission { value } => {
                match self
                    .auth_runtime
                    .login_with_token(&mut self.auth, SecretString::new(value))
                {
                    Ok(event) => self.auth_response(event),
                    Err(err) => map_auth_flow_error(err),
                }
            }
            Request::TwoFactorCodeSubmission {
                value,
                trust_device,
                recovery_code,
            } => {
                let response = if self
                    .auth
                    .snapshot()
                    .pending_challenge
                    .as_ref()
                    .map(|challenge| challenge.token.expose_secret().is_empty())
                    .unwrap_or(false)
                {
                    match self.pending_password_auth.clone() {
                        Some(pending) => self.auth_runtime.submit_two_factor_code_with_password(
                            &mut self.auth,
                            pending.username,
                            pending.password,
                            SecretString::new(value),
                        ),
                        None => Err(AuthFlowError::Session(
                            pcloud_auth::SessionManagerError::NoPendingChallenge,
                        )),
                    }
                } else {
                    self.auth_runtime.submit_two_factor_code(
                        &mut self.auth,
                        SecretString::new(value),
                        trust_device,
                        recovery_code,
                    )
                };

                match response {
                    Ok(event) => self.auth_response(event),
                    Err(err) => map_auth_flow_error(err),
                }
            }
            Request::CryptoUnlock { password } => {
                self.unlock_crypto(SecretString::new(password.into_string()))
            }
            Request::CryptoSetup { password, hint } => {
                self.setup_crypto(SecretString::new(password.into_string()), hint)
            }
            Request::CryptoSetupV2 {
                backend,
                acknowledge_not_interop,
                password,
                hint,
            } => self.setup_crypto_v2(
                backend,
                acknowledge_not_interop,
                SecretString::new(password.into_string()),
                hint,
            ),
            Request::CryptoGetFolderKey { folder_id } => self.crypto_get_folder_key(folder_id),
            Request::CryptoGetFileKey { file_id } => self.crypto_get_file_key(file_id),
            Request::CryptoChangePassword {
                old_password,
                new_password,
                hint,
                code,
                flags,
            } => self.change_crypto_password(
                SecretString::new(old_password.into_string()),
                SecretString::new(new_password.into_string()),
                hint,
                code,
                flags,
            ),
            Request::CryptoChangePasswordUnlocked {
                new_password,
                hint,
                code,
                flags,
            } => self.change_crypto_password_unlocked(
                SecretString::new(new_password.into_string()),
                hint,
                code,
                flags,
            ),
            Request::CryptoMkdir {
                name,
                parent_folder_id,
                local_folder_id,
            } => self.crypto_mkdir(name, parent_folder_id, local_folder_id),
            Request::AuthPersistence { enabled } => self.set_auth_persistence(enabled),
            Request::SyncRootAdd {
                local_path,
                remote_path,
                sync_type,
            } => self.add_sync_root(local_path, remote_path, sync_type),
            Request::SyncRootRemove { sync_id } => self.remove_sync_root(sync_id),
            Request::SyncRootPause { sync_id } => self.pause_sync_root(sync_id),
            Request::SyncRootResume { sync_id } => self.resume_sync_root(sync_id),
            Request::SyncRootChangeType { sync_id, sync_type } => {
                self.change_sync_root_type(sync_id, sync_type)
            }
            Request::GetSyncSuggestions { path, max } => {
                self.suggest_sync_folders_at(path, max.unwrap_or(5))
            }
            Request::IsFolderSyncable { path } => self.check_folder_syncable(path),
            Request::ShowPublicLink { code } => self.show_public_link(code),
            Request::DeletePublicLink { link_id } => self.delete_public_link(link_id),
            Request::DeletePublicLinkByCode { code } => self.delete_public_link_by_code(code),
            Request::CreateFilePublicLink { path } => self.create_file_public_link(path),
            Request::CreateFolderPublicLink { path } => self.create_folder_public_link(path),
            Request::ChangePublicLinkExpire { link_id, expire } => {
                self.change_public_link_expire(link_id, expire)
            }
            Request::ChangePublicLinkPassword { link_id, password } => {
                // ncx.66: wire transit is `RedactedString`; destructure
                // immediately into `SecretString` so every daemon-side
                // handler, backend call and proto boundary below zeroizes
                // on drop and redacts in Debug.
                self.change_public_link_password(
                    link_id,
                    password.map(|p| SecretString::new(p.into_string())),
                )
            }
            Request::ChangePublicLinkUpload { link_id, policy } => {
                self.change_public_link_upload(link_id, policy)
            }
            Request::CreateUploadLink {
                path,
                comment,
                expire,
                maxspace,
                maxfiles,
            } => self.create_upload_link(path, comment, expire, maxspace, maxfiles),
            Request::DeleteUploadLink { upload_link_id } => self.delete_upload_link(upload_link_id),
            Request::CreateTreePublicLink {
                name,
                root_folder_id,
                folder_ids_csv,
                file_ids_csv,
                expire,
                maxdownloads,
                maxtraffic,
            } => self.create_tree_public_link(
                name,
                root_folder_id,
                folder_ids_csv,
                file_ids_csv,
                expire,
                maxdownloads,
                maxtraffic,
            ),
            Request::ListPublicLinkAccess { link_id } => self.list_public_link_access(link_id),
            Request::AddPublicLinkAccess { link_id, email } => {
                self.add_public_link_access(link_id, email)
            }
            Request::RemovePublicLinkAccess {
                link_id,
                receiver_id,
            } => self.remove_public_link_access(link_id, receiver_id),
            Request::ListBookmarks => self.list_bookmarks(),
            Request::RemoveBookmark { code, location_id } => {
                self.remove_bookmark(code, location_id)
            }
            Request::ChangeBookmark {
                code,
                location_id,
                name,
                description,
            } => self.change_bookmark(code, location_id, name, description),
            Request::ShareFolder {
                folder_id,
                name,
                mail,
                message,
                permissions_bits,
                hint,
            } => self.share_folder(folder_id, name, mail, message, permissions_bits, hint),
            Request::CancelShareRequest { share_request_id } => {
                self.cancel_share_request(share_request_id)
            }
            Request::DeclineShareRequest { share_request_id } => {
                self.decline_share_request(share_request_id)
            }
            Request::AcceptShareRequest {
                share_request_id,
                to_folder_id,
                name,
            } => self.accept_share_request(share_request_id, to_folder_id, name),
            Request::RemoveShare { share_id } => self.remove_share(share_id),
            Request::ModifyShare {
                share_id,
                permissions_bits,
            } => self.modify_share(share_id, permissions_bits),
            Request::AccountStopShare {
                user_share_ids,
                team_share_ids,
            } => self.account_stop_share(user_share_ids, team_share_ids),
            Request::AccountModifyShare {
                user_shares,
                team_shares,
            } => self.account_modify_share(user_shares, team_shares),
            Request::AccountTeamShare {
                folder_id,
                name,
                team_id,
                message,
                permissions_bits,
                hint,
            } => self.account_team_share(folder_id, name, team_id, message, permissions_bits, hint),
            Request::ValueGet { name, kind } => self.value_get(&name, kind),
            Request::ValueSet { name, value } => self.value_set(&name, value),
            Request::ValueHas { name, kind } => self.value_has(&name, kind),
            Request::MarkNotificationsRead { upto_id } => self.mark_notifications_read(upto_id),
            Request::AuditVerifyChain { range } => self.audit_verify_chain(range),
            Request::Mount { path } => self.mount_filesystem(&path),
            Request::Unmount => self.unmount_filesystem(),
            Request::MountForceUnmount { path } => self.mount_control.force_unmount_path(&path),
            Request::CreateRemoteFolder {
                parent_folder_id,
                name,
                path,
                check_and_create,
            } => self.create_remote_folder(parent_folder_id, name, path, check_and_create),
            Request::SessionStatus => self.session_status(),
            Request::RunLocalScan => self.run_localscan(),
            Request::SendPublink {
                code,
                mails,
                message,
            } => self.send_publink(code, mails, message),
            Request::GetFolderIdByPath { path } => self.get_folder_id_by_path(path),
            Request::GetFolderFlags { path } => self.get_folder_flags(path),
            Request::GetFolderOwnerId { path } => self.get_folder_owner_id(path),
            Request::FilesystemStatus { path } => self.filesystem_status(path),
            Request::StatPath { path } => self.stat_path(path),
            // bd-smbr-pcloud P2: filesystem ops needed by the smbr
            // pcloud-rs VFS plugin. The IPC surface is locked here so
            // smbr can build against it; the live handlers will wire
            // through `pcloud_backends::folder_backend` /
            // `transfer_backend` (already implemented). For now they
            // return Unavailable so an early adopter sees an explicit
            // tracker pointer instead of a silent panic.
            Request::ListFolderByPath { path } => self.list_folder_by_path(path),
            Request::FileDeleteByPath { path } => self.file_delete_by_path(path),
            Request::FolderDeleteByPath { path, recursive } => {
                self.folder_delete_by_path(path, recursive)
            }
            Request::FolderDeleteById {
                folder_id,
                recursive,
            } => self.folder_delete_by_id(folder_id, recursive),
            Request::RenamePath { from, to } => self.rename_path(from, to),
            Request::FileHistory { path, limit } => self.file_history(path, limit),
            // Backup/snapshot lifecycle (zstd + SHA3 sidecar default,
            // optional GPG envelope). The full pipeline is now wired
            // through `pcloud_backends::snapshot`.
            Request::BackupSnapshot {
                action,
                path,
                gpg_recipient,
                yes,
                retention_days,
                zstd_level,
            } => self.handle_backup_snapshot(
                action,
                path,
                gpg_recipient,
                yes,
                retention_days,
                zstd_level,
            ),
            // H14 PR4 — integrity sweeper request surface. See
            // bd-1du.4.6.1.
            Request::IntegrityRunOnce => self.integrity_run_once(),
            Request::IntegritySkip { path } => self.integrity_skip(path),
            // Operator-visible upload-session control surface
            // (pause/resume/cancel/list). See
            // `pcloud_backends::upload_sessions` for the state machine
            // and `docs/book/src/operations/partial-transfers.md` for
            // operator-facing usage notes.
            Request::UploadCreate {
                local_path,
                remote_name,
                parent_folder_id,
                total_bytes,
                conflict_mode,
            } => self.upload_session_create(
                local_path,
                remote_name,
                parent_folder_id,
                total_bytes,
                conflict_mode,
            ),
            Request::UploadPause { session_id } => self.upload_session_pause(session_id),
            Request::UploadResume { session_id } => self.upload_session_resume(session_id),
            Request::UploadCancel { session_id } => self.upload_session_cancel(session_id),
            Request::UploadList => self.upload_session_list(),
            Request::ConflictList => self.list_conflicts(),
            Request::ConflictResolve { path, policy } => self.resolve_conflict(path, policy),
            Request::LostPassword { email } => self.lost_password(email),
            Request::VerifyEmailRestricted { verify_token } => {
                self.verify_email_restricted(verify_token)
            }
            Request::AccountChangePassword {
                current_password,
                new_password,
            } => self.account_change_password(
                SecretString::new(current_password.into_string()),
                SecretString::new(new_password.into_string()),
            ),
            Request::AccountRegister {
                email,
                password,
                terms_accepted,
            } => self.account_register(
                email,
                SecretString::new(password.into_string()),
                terms_accepted,
            ),
            Request::GetFileLink { file_id } => self.get_file_link_ipc(file_id),
            Request::DownloadFile {
                file_id,
                local_path,
            } => self.download_file_ipc(file_id, local_path),
            Request::DeleteBackup { backup_id } => self.delete_backup_ipc(backup_id),
            Request::SetApiServer {
                location_id,
                binapi,
            } => self.set_api_server_ipc(location_id, binapi),
            Request::SetLanguage { language } => self.set_language_ipc(language),
            Request::UploadWriteFromFile {
                upload_session_id,
                source_fileid,
                source_hash,
                offset,
                count,
            } => self.upload_write_from_file_ipc(
                upload_session_id,
                source_fileid,
                source_hash,
                offset,
                count,
            ),
            Request::CreateTreePublicLinkFromPaths {
                name,
                paths,
                expires,
            } => self.create_tree_public_link_from_paths_ipc(name, paths, expires),
            Request::CreateBackup {
                name,
                root_folder_id,
                local_path,
                parent_folder_name,
            } => self.create_backup_ipc(name, root_folder_id, local_path, parent_folder_name),
            Request::StopDevice { device_folder_id } => self.stop_device_ipc(device_folder_id),
            Request::DeleteBackupDevice => self.delete_backup_device_ipc(),
            // `Request` is `#[non_exhaustive]`: unknown variants from a
            // newer client build are reported rather than silently matched.
            _ => Response {
                status: ResponseStatus::InvalidRequest,
                message: "unsupported ipc request (newer client than daemon?)".to_owned(),
            },
        }
    }

    /// Trigger an immediate local-scan wakeup on the engine scheduler.
    /// Mirrors C `psync_run_localscan` (`pclsync/psynclib.c:886`), which
    /// calls `psync_wake_localscan`.
    fn run_localscan(&mut self) -> Response {
        let count = self.engine.wake_localscan();
        self.audited_response(
            "sync.localscan.wake",
            Some(format!("count={count}")),
            format!("local scan wake signalled (count={count})"),
        )
    }

    /// Mail an existing public link `code` to one or more recipients.
    /// Mirrors C `psync_send_publink` (`pclsync/psynclib.c:2217`).
    fn send_publink(&mut self, code: String, mails: String, message: String) -> Response {
        if code.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "send public link requires a non-empty code".to_owned(),
            };
        }
        if mails.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "send public link requires at least one recipient".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "sending a public link requires an authenticated session".to_owned(),
                };
            }
        };
        // Recipient count only — the comma-separated mail list is PII and
        // must not land in the audit log. The public-link `code` itself is
        // a short server-issued identifier already exposed in user-facing
        // URLs, so logging it is safe.
        let recipient_count = mails
            .split(',')
            .map(str::trim)
            .filter(|addr| !addr.is_empty())
            .count();
        let result =
            self.public_link_runtime
                .send_publink(auth_token, code.clone(), mails, message);
        // `auth_token` is a `SecretString` that was `clone_secret`-ed
        // above; it zeroizes on Drop when this scope ends.
        match result {
            Ok(()) => self.audited_response(
                "publinks.send",
                Some(format!("code={code} recipients={recipient_count}")),
                format!("public link sent: code=\"{code}\" recipients={recipient_count}"),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    /// Resolve an absolute pCloud-drive path to its folder id.
    /// Mirrors C `psync_get_fsfolderid_by_path`
    /// (`pclsync/psynclib.c:2170`).
    ///
    /// Enterprise rules enforced here:
    /// - the auth token is obtained via `SessionManager::snapshot` and
    ///   immediately wrapped in a fresh `SecretString` that is zeroised on
    ///   drop (the resolver keeps it in a `SecretString` field),
    /// - the token never appears in audit details or response messages,
    /// - on miss the handler returns a typed error instead of the C
    ///   `PSYNC_INVALID_FSFOLDERID` (`0`) sentinel.
    pub fn get_folder_id_by_path(&mut self, path: String) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "get-folder-id-by-path requires a non-empty absolute path".to_owned(),
            };
        }
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "get-folder-id-by-path requires an authenticated session".to_owned(),
            };
        };
        let resolver = self.public_link_runtime.path_resolver(auth_token);
        match resolver.get_folder_id_by_path(&path) {
            Ok(id) => self.audited_response(
                "folder.id_by_path",
                Some(format!("path={path}")),
                format!("folder id for {path:?}: {}", id.get()),
            ),
            Err(err) => map_path_resolve_error(err),
        }
    }

    /// Read folder flags / permissions / sharing / encryption view for
    /// an absolute pCloud-drive path. Mirrors C
    /// `psync_get_fsfolderflags_by_id` (`pclsync/psynclib.c:2176`) and
    /// the `flags`+`permissions` out-params of
    /// `pfs_fldr_idperm_by_path` (`pfsfolder.c:342`).
    /// Resolve a remote path and return its folder flags bitmap
    /// (encryption, public, backup, etc.) as a response payload.
    pub fn get_folder_flags(&mut self, path: String) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "get-folder-flags requires a non-empty absolute path".to_owned(),
            };
        }
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "get-folder-flags requires an authenticated session".to_owned(),
            };
        };
        let resolver = self.public_link_runtime.path_resolver(auth_token);
        match resolver.get_folder_flags(&path) {
            Ok(flags) => {
                // Emit a structured single-line key=value message; no
                // secrets, no absolute filesystem paths beyond the caller's
                // own input.
                let perm_str = flags
                    .permissions
                    .map(|p| format!("{p}"))
                    .unwrap_or_else(|| "unknown".to_owned());
                let message = format!(
                    "folder flags for {path:?}: permissions={perm_str} encrypted={} shared={} readonly={}",
                    flags.encrypted, flags.shared, flags.readonly,
                );
                self.audited_response(
                    "folder.flags_by_path",
                    Some(format!("path={path}")),
                    message,
                )
            }
            Err(err) => map_path_resolve_error(err),
        }
    }

    /// Read the owner user id of a folder by absolute pCloud-drive
    /// path. Mirrors C `psync_get_folder_ownerid`
    /// (`pclsync/psynclib.c:2088`).
    /// Resolve a remote path and return the owning user id of the
    /// containing folder.
    pub fn get_folder_owner_id(&mut self, path: String) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "get-folder-owner-id requires a non-empty absolute path".to_owned(),
            };
        }
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "get-folder-owner-id requires an authenticated session".to_owned(),
            };
        };
        let resolver = self.public_link_runtime.path_resolver(auth_token);
        match resolver.get_folder_owner_id(&path) {
            Ok(user_id) => self.audited_response(
                "folder.owner_by_path",
                Some(format!("path={path}")),
                format!("folder owner user_id for {path:?}: {}", user_id.get()),
            ),
            Err(err) => map_path_resolve_error(err),
        }
    }

    /// Classify an absolute local path against the daemon's sync-root +
    /// engine state. Mirrors C `psync_filesystem_status`
    /// (`pclsync/psynclib.c:1903`). The response message is one of the
    /// four C tokens `INSYNC`, `INPROG`, `NOSYNC`, `INVSYNC` so
    /// downstream consumers see an exact parity string.
    /// Return per-path filesystem sync status (pending/syncing/synced/
    /// error) for the provided local path.
    pub fn filesystem_status(&mut self, path: String) -> Response {
        use crate::path_resolver::{
            FilesystemStatusInputs, FsPathStatus, SyncRootView, filesystem_status as classify,
        };
        use pcloud_model::sync::PlannedOperation;

        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "filesystem-status requires a non-empty absolute local path".to_owned(),
            };
        }

        let sync_roots: Vec<SyncRootView<'_>> = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter()
            .map(|root| SyncRootView {
                sync_id: root.sync_id.get(),
                local_path: root.local_path.as_str(),
                paused: root.paused,
            })
            .collect();

        let paused_from_engine: Vec<u64> = sync_roots
            .iter()
            .filter_map(|view| {
                let id = pcloud_model::ids::SyncId::new(view.sync_id);
                if self.engine.is_sync_root_paused(id) {
                    Some(view.sync_id)
                } else {
                    None
                }
            })
            .collect();

        let mut queued_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        let mut errored_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
        for op in self.engine.scheduler.queued_operations.iter() {
            if matches!(op, PlannedOperation::Conflict { .. }) {
                errored_ids.insert(op.sync_id().get());
            } else {
                queued_ids.insert(op.sync_id().get());
            }
        }
        let queued_vec: Vec<u64> = queued_ids.into_iter().collect();
        let errored_vec: Vec<u64> = errored_ids.into_iter().collect();

        let inputs = FilesystemStatusInputs {
            sync_roots: &sync_roots,
            paused_sync_ids: &paused_from_engine,
            queued_sync_ids: &queued_vec,
            errored_sync_ids: &errored_vec,
        };

        let status = classify(Path::new(&path), inputs);
        let token = match status {
            FsPathStatus::InSync => "INSYNC",
            FsPathStatus::InProgress => "INPROG",
            FsPathStatus::NoSync => "NOSYNC",
            FsPathStatus::Invalid => "INVSYNC",
        };
        self.audited_response(
            "fs.status_by_path",
            Some(format!("path={path} status={token}")),
            token.to_owned(),
        )
    }

    /// Stat an absolute pCloud-drive path: resolve through local
    /// metadata cache, fall back to API. Mirrors C `psync_stat_path`
    /// (`pclsync/psynclib.h:743`, `pfolder.c:734`).
    pub fn stat_path(&mut self, path: String) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "stat requires a non-empty absolute pCloud-drive path".to_owned(),
            };
        }

        // Try local metadata cache first.
        let db_path = &self.store.db_path;
        if let Ok(conn) = rusqlite::Connection::open(db_path) {
            if let Ok(Some(record)) =
                pcloud_store::FileMetadataRepository::resolve_path(&conn, &path)
            {
                let payload = pcloud_ipc::StatPathPayload {
                    file_id: record.file_id,
                    parent_folder_id: record.parent_folder_id,
                    name: record.name,
                    size: record.size,
                    hash: record.hash,
                    modified: record.modified,
                    created: record.created,
                    is_folder: record.is_folder,
                    source: "cache".to_owned(),
                };
                return self.audited_response(
                    "fs.stat_path",
                    Some(format!("path={path} source=cache")),
                    serde_json::to_string(&payload).unwrap_or_default(),
                );
            }
        }

        // Fall back to API: resolve folder id via the path resolver.
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "stat requires authentication (no cached metadata and no auth token)"
                    .to_owned(),
            };
        };
        let resolver = self.public_link_runtime.path_resolver(auth_token);
        match resolver.get_folder_id_by_path(&path) {
            Ok(folder_id) => {
                let leaf = path.rsplit('/').next().unwrap_or(&path).to_owned();
                let payload = pcloud_ipc::StatPathPayload {
                    file_id: folder_id.get(),
                    parent_folder_id: 0,
                    name: leaf,
                    size: 0,
                    hash: String::new(),
                    modified: 0,
                    created: 0,
                    is_folder: true,
                    source: "api".to_owned(),
                };
                self.audited_response(
                    "fs.stat_path",
                    Some(format!("path={path} source=api")),
                    serde_json::to_string(&payload).unwrap_or_default(),
                )
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("stat_path: not in local cache, API fallback failed: {err}"),
            },
        }
    }

    /// Mount the pCloud filesystem at `mountpoint`.
    ///
    /// bd-1du.4.e sub-task 2: delegates to [`MountControl`], which
    /// re-validates the mountpoint (ownership, permissions, non-empty,
    /// `allow_other`=false) before invoking the FUSE session.
    ///
    /// When authenticated on a networked transport, swaps in a composed
    /// [`pcloud_fs::fuser_shim::PcloudFsShim`] factory before mounting so
    /// FUSE operations actually hit the real remote. When auth or transport
    /// are unavailable, falls back to the `NullFuseAdapter` (`ENOSYS`) so
    /// the mount lifecycle is still testable.
    /// Mount the pCloud FUSE filesystem at `mountpoint`. Delegates to
    /// [`crate::mount_runtime::MountControl::mount`] after refreshing
    /// the adapter factory with the current auth state.
    ///
    // Mounts a fully wired ProtoFuseAdapter that serves pCloud files via
    // the pCloud binary protocol. All FUSE operations (lookup, readdir,
    // read, write, flush, fsync, create, unlink, rename, mkdir, rmdir)
    // are forwarded through the adapter to the pCloud API.
    pub fn mount_filesystem(&mut self, mountpoint: &Path) -> Response {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        self.try_install_pcloud_shim_factory();
        self.mount_control.mount(mountpoint)
    }

    /// Return the revision history of a file by absolute remote path.
    ///
    /// Mirrors the C `listrevisions` wire command
    /// (`pclsync/pnetlibs.c:2481`, `download_file_revisions`).
    ///
    /// # Pluggable provider (bd-1du.10)
    ///
    /// pCloud's public API catalogue does not currently document a
    /// third-party-accessible `listrevisions` endpoint. Instead of a
    /// dead-end `Unavailable` the daemon wires a `RevisionProvider`
    /// selected from config:
    ///
    /// - `[file_history].revision_url` **unset** →
    ///   [`pcloud_proto::revision_provider::NullRevisionProvider`]
    ///   returns a structured JSON response with
    ///   `{"status":"not_configured","message":…,"next":…}` so the CLI
    ///   can render an actionable remediation hint. The response status
    ///   is [`ResponseStatus::Unavailable`] (exit 6).
    /// - `[file_history].revision_url` **set** → the daemon constructs
    ///   an HTTP provider (feature `file-history-http` on `pcloud-proto`)
    ///   and forwards the call. On success the response carries
    ///   `{"revisions":[…],"count":N}` as JSON in [`Response::message`].
    ///
    /// Empty / whitespace paths are refused with
    /// [`ResponseStatus::InvalidRequest`] before the provider is
    /// consulted.
    /// bd-smbr-pcloud P4.3 — resolve an absolute pCloud-drive path
    /// to its kind + numeric id, by reading the local metadata cache
    /// only. Returns `Ok(None)` when the path is not present in the
    /// cache so the caller can decide whether to fall back to the
    /// API or surface "not found".
    ///
    /// Mirrors the cache-first half of [`Self::stat_path`]; the
    /// API-fallback half is intentionally not duplicated here
    /// because the only kind-aware API is the same `listfolder`
    /// already used by [`Self::list_folder_by_path`], and the new
    /// FS-by-path handlers prefer to *fail closed* on a cache miss
    /// rather than implicitly issue listfolder per delete (which
    /// would let a malicious caller fingerprint the parent folder
    /// via timing).
    fn resolve_kind_by_path(
        &self,
        path: &str,
    ) -> Result<Option<(u64, bool)>, ResponseStatus> {
        let db_path = &self.store.db_path;
        let conn = match rusqlite::Connection::open(db_path) {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };
        match pcloud_store::FileMetadataRepository::resolve_path(&conn, path) {
            Ok(Some(record)) => Ok(Some((record.file_id, record.is_folder))),
            Ok(None) => Ok(None),
            Err(_) => Ok(None),
        }
    }

    /// bd-smbr-pcloud P4 — list folder children by absolute pCloud
    /// drive path. Resolves `path` via
    /// [`crate::folder_backend::FolderRuntime::list_folder_contents`]
    /// and emits a JSON-serialised `Vec<pcloud_ipc::ListFolderEntry>`
    /// in [`Response::message`].
    ///
    /// The mapping from `RemoteFolderEntry` → `ListFolderEntry` is
    /// lossy on two fields the wire schema declares but the
    /// underlying type does not yet expose:
    /// * `hash` — defaults to the empty string. Folders never have
    ///   a content hash (consistent with the schema doc); files
    ///   currently report empty pending a richer extractor in
    ///   `pcloud-proto::folder_api`.
    /// * `created` — defaults to `0` (treated as "unknown" by the
    ///   smbr plugin, which falls back to `modified`).
    /// Both are tracked under `bd-smbr-pcloud P4 follow-up`; the IPC
    /// wire shape stays stable while we close the gaps.
    pub fn list_folder_by_path(&mut self, path: String) -> Response {
        if !path.starts_with('/') {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "list-folder-by-path requires an absolute path starting with '/'"
                    .to_owned(),
            };
        }
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "list-folder-by-path requires an authenticated session".to_owned(),
            };
        };
        match self.folder_runtime.list_folder_contents(auth_token, &path) {
            Ok(listing) => {
                let entries: Vec<pcloud_ipc::ListFolderEntry> = listing
                    .entries
                    .iter()
                    .map(|entry| pcloud_ipc::ListFolderEntry {
                        file_id: entry
                            .folder_id
                            .or(entry.file_id)
                            .unwrap_or(0),
                        name: entry.name.clone(),
                        size: entry.size.unwrap_or(0),
                        // P4 follow-up: enrich the proto-level
                        // entry to expose `hash` + `created` so we
                        // can fill them in here without a lossy
                        // default. Tracker: bd-smbr-pcloud P4 fu.
                        hash: String::new(),
                        modified: entry.modified.unwrap_or(0) as i64,
                        created: 0,
                        is_folder: entry.is_folder,
                    })
                    .collect();
                let count = entries.len();
                let body = match serde_json::to_string(&entries) {
                    Ok(s) => s,
                    Err(err) => {
                        return Response {
                            status: ResponseStatus::InternalError,
                            message: format!(
                                "list-folder-by-path: JSON serialise failed: {err}"
                            ),
                        };
                    }
                };
                self.audited_response(
                    "fs.list_folder_by_path",
                    Some(format!("path={path} entries={count}")),
                    body,
                )
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!(
                    "list-folder-by-path: {path:?}: {err}"
                ),
            },
        }
    }

    /// bd-smbr-pcloud P4.3 — delete a remote file by absolute
    /// pCloud-drive path. Resolves `path` against the local
    /// metadata cache (cache-only, see [`Self::resolve_kind_by_path`])
    /// then dispatches
    /// [`crate::transfer_backend::TransferRuntime::delete_file_by_id`].
    /// Idempotent on missing path: SMB clients with a stale dirent
    /// cache do not get a noisy failure for a path their cached
    /// `readdir` thinks still exists.
    pub fn file_delete_by_path(&mut self, path: String) -> Response {
        if !path.starts_with('/') {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "file-delete-by-path requires an absolute path starting with '/'"
                    .to_owned(),
            };
        }
        let resolved = match self.resolve_kind_by_path(&path) {
            Ok(opt) => opt,
            Err(status) => return Response {
                status,
                message: format!("file-delete-by-path: resolve failed for {path:?}"),
            },
        };
        let Some((file_id, is_folder)) = resolved else {
            return self.audited_response(
                "fs.file_delete_by_path",
                Some(format!("path={path} result=already_absent")),
                format!("file-delete-by-path: {path:?} not present (idempotent)"),
            );
        };
        if is_folder {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!(
                    "file-delete-by-path: {path:?} resolves to a folder \
                     (use folder-delete-by-path)"
                ),
            };
        }
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "file-delete-by-path requires an authenticated session".to_owned(),
            };
        };
        match self.transfer_runtime.delete_file_by_id(auth_token, file_id) {
            Ok(()) => self.audited_response(
                "fs.file_delete_by_path",
                Some(format!("path={path} file_id={file_id}")),
                format!("file-delete-by-path: deleted {path:?} (file_id={file_id})"),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("file-delete-by-path: API delete failed for {path:?}: {err}"),
            },
        }
    }

    /// bd-smbr-pcloud P4.3 — delete a remote folder by absolute
    /// pCloud-drive path. Picks between `deletefolder` and
    /// `deletefolderrecursive` based on `recursive`. Idempotent on
    /// missing path.
    pub fn folder_delete_by_path(&mut self, path: String, recursive: bool) -> Response {
        if !path.starts_with('/') {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "folder-delete-by-path requires an absolute path starting with '/'"
                    .to_owned(),
            };
        }
        let resolved = match self.resolve_kind_by_path(&path) {
            Ok(opt) => opt,
            Err(status) => return Response {
                status,
                message: format!("folder-delete-by-path: resolve failed for {path:?}"),
            },
        };
        let Some((folder_id, is_folder)) = resolved else {
            return self.audited_response(
                "fs.folder_delete_by_path",
                Some(format!(
                    "path={path} result=already_absent recursive={recursive}"
                )),
                format!("folder-delete-by-path: {path:?} not present (idempotent)"),
            );
        };
        if !is_folder {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!(
                    "folder-delete-by-path: {path:?} resolves to a file \
                     (use file-delete-by-path)"
                ),
            };
        }
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "folder-delete-by-path requires an authenticated session".to_owned(),
            };
        };
        match self
            .folder_runtime
            .delete_folder_by_id(auth_token, folder_id, recursive)
        {
            Ok(()) => self.audited_response(
                "fs.folder_delete_by_path",
                Some(format!(
                    "path={path} folder_id={folder_id} recursive={recursive}"
                )),
                format!(
                    "folder-delete-by-path: deleted {path:?} \
                     (folder_id={folder_id}, recursive={recursive})"
                ),
            ),
            Err(err) => match &err {
                pcloud_proto::FolderApiError::Result { result: 2005, .. } => {
                    self.audited_response(
                        "fs.folder_delete_by_path",
                        Some(format!("path={path} result=already_absent_api")),
                        format!(
                            "folder-delete-by-path: {path:?} not present (API idempotent)"
                        ),
                    )
                }
                pcloud_proto::FolderApiError::Result { result: 2003, .. } => Response {
                    status: ResponseStatus::Conflict,
                    message: format!(
                        "folder-delete-by-path: {path:?} not empty (recursive={recursive})"
                    ),
                },
                _ => Response {
                    status: ResponseStatus::InternalError,
                    message: format!(
                        "folder-delete-by-path: API delete failed for {path:?}: {err}"
                    ),
                },
            },
        }
    }

    /// `FolderDeleteById` IPC handler — delete a remote folder by
    /// numeric folder id. Mirrors C `task_deletefolder` /
    /// `task_deletefolderrec`. Authenticated. Idempotent on
    /// `Folder Not Found`.
    pub fn folder_delete_by_id(&mut self, folder_id: u64, recursive: bool) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "folder-delete-by-id requires an authenticated session".to_owned(),
                };
            }
        };
        match self
            .folder_runtime
            .delete_folder_by_id(auth_token, folder_id, recursive)
        {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!("folder deleted: folder_id={folder_id}, recursive={recursive}"),
            },
            Err(err) => match &err {
                pcloud_proto::FolderApiError::Result { result: 2005, .. } => Response {
                    // pCloud `2005 Folder Not Found` — idempotent path.
                    status: ResponseStatus::Ok,
                    message: format!("folder already absent: folder_id={folder_id} ({err})"),
                },
                pcloud_proto::FolderApiError::Result { result: 2003, .. } => Response {
                    // pCloud `2003 Directory Not Empty` (recursive=false).
                    status: ResponseStatus::Conflict,
                    message: format!(
                        "folder is not empty: folder_id={folder_id} ({err}); \
                         retry with recursive=true"
                    ),
                },
                _ => Response {
                    status: ResponseStatus::InternalError,
                    message: format!("folder delete failed: {err}"),
                },
            },
        }
    }

    /// bd-smbr-pcloud P4.3 — rename or move a file/folder identified
    /// by absolute pCloud-drive path. Probes `from` to determine
    /// file vs folder, then dispatches
    /// [`crate::transfer_backend::TransferRuntime::rename_file_by_id`]
    /// or
    /// [`crate::folder_backend::FolderRuntime::rename_folder_by_id`].
    /// Cross-folder moves are supported when the destination's
    /// parent folder differs from the source's; the parent must
    /// already exist.
    pub fn rename_path(&mut self, from: String, to: String) -> Response {
        if !from.starts_with('/') || !to.starts_with('/') {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "rename-path requires both `from` and `to` to be absolute paths"
                    .to_owned(),
            };
        }
        let (to_parent_path, to_name) = match split_parent_and_leaf(&to) {
            Some(parts) => parts,
            None => return Response {
                status: ResponseStatus::InvalidRequest,
                message: format!("rename-path: cannot extract parent+leaf from {to:?}"),
            },
        };
        let resolved = match self.resolve_kind_by_path(&from) {
            Ok(opt) => opt,
            Err(status) => return Response {
                status,
                message: format!("rename-path: resolve failed for from={from:?}"),
            },
        };
        let Some((source_id, is_folder)) = resolved else {
            return Response {
                status: ResponseStatus::InternalError,
                message: format!(
                    "rename-path: {from:?} not in metadata cache (cannot determine kind)"
                ),
            };
        };
        let resolved_parent = match self.resolve_kind_by_path(&to_parent_path) {
            Ok(opt) => opt,
            Err(status) => return Response {
                status,
                message: format!(
                    "rename-path: resolve failed for parent of {to:?} ({to_parent_path:?})"
                ),
            },
        };
        let to_folder_id = match resolved_parent {
            Some((id, true)) => id,
            Some((_, false)) => return Response {
                status: ResponseStatus::Conflict,
                message: format!(
                    "rename-path: parent of {to:?} ({to_parent_path:?}) is a file, not a folder"
                ),
            },
            None => return Response {
                status: ResponseStatus::InternalError,
                message: format!(
                    "rename-path: parent of {to:?} ({to_parent_path:?}) not in metadata cache"
                ),
            },
        };
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "rename-path requires an authenticated session".to_owned(),
            };
        };
        let outcome = if is_folder {
            self.folder_runtime
                .rename_folder_by_id(auth_token, source_id, to_folder_id, &to_name)
                .map_err(|e| format!("renamefolder failed: {e}"))
        } else {
            self.transfer_runtime
                .rename_file_by_id(auth_token, source_id, to_folder_id, &to_name)
                .map_err(|e| format!("renamefile failed: {e}"))
        };
        match outcome {
            Ok(()) => self.audited_response(
                "fs.rename_path",
                Some(format!(
                    "from={from} to={to} kind={} id={source_id} to_parent={to_folder_id}",
                    if is_folder { "folder" } else { "file" }
                )),
                format!("rename-path: {from:?} → {to:?}"),
            ),
            Err(msg) => Response {
                status: ResponseStatus::InternalError,
                message: format!("rename-path: {from:?} → {to:?}: {msg}"),
            },
        }
    }

    /// Return the revision history of a remote file by absolute path.
    /// Mirrors C `psync_listrevisions`. The current implementation
    /// dispatches through the optional HTTP provider on
    /// `pcloud-proto`; when the provider is not enabled, returns
    /// [`ResponseStatus::Unavailable`] with a tracker pointer so
    /// callers see the honest scope.
    pub fn file_history(&mut self, path: String, limit: Option<u32>) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "file-history requires a non-empty absolute remote path".to_owned(),
            };
        }

        let provider = self.build_revision_provider();
        match provider.list_revisions(&path) {
            Ok(mut revisions) => {
                if let Some(cap) = limit {
                    revisions.truncate(cap as usize);
                }
                let count = revisions.len();
                let payload = serde_json::json!({
                    "revisions": revisions,
                    "count": count,
                });
                Response {
                    status: ResponseStatus::Ok,
                    message: serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_owned()),
                }
            }
            Err(err) => Response {
                status: revision_err_to_status(&err),
                message: render_revision_error_payload(&err, &path),
            },
        }
    }

    /// Build the configured revision provider.
    ///
    /// When `[file_history].revision_url` is unset (the default), this
    /// returns a boxed
    /// [`pcloud_proto::revision_provider::NullRevisionProvider`]. When
    /// the `file-history-http` feature is enabled on `pcloud-proto` and
    /// the URL is set, an HTTP provider is constructed instead. In the
    /// default build (feature off) the Null provider is always returned
    /// even when the URL is configured, which keeps the daemon's
    /// default response ("not configured") honest.
    fn build_revision_provider(
        &self,
    ) -> Box<dyn pcloud_proto::revision_provider::RevisionProvider> {
        // The URL is currently only consumed by the `file-history-http`
        // branch. Underscore-prefix the binding so non-feature builds do
        // not trip the `unused_variables` lint.
        let _url = self
            .config
            .file_history
            .revision_url
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());

        Box::new(pcloud_proto::revision_provider::NullRevisionProvider::new())
    }

    /// Compose a real live-FUSE adapter factory and install it on
    /// [`MountControl`] if, and only if, (a) auth is live and (b) a
    /// networked transport is available. Otherwise the existing factory
    /// (default `NullFuseAdapter`) is left untouched.
    ///
    /// On Linux the factory returns a [`pcloud_fs::fuser_shim::PcloudFsShim`]
    /// wrapped around a [`pcloud_fs::fuse_adapter::ProtoFuseAdapter`].
    /// On macOS the `fuser` crate is not available, so the factory returns
    /// the bare `ProtoFuseAdapter` and [`pcloud_fs::mount_service::MountService`]
    /// dispatches through the fuse-t FFI instead of `mount_fuser`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn try_install_pcloud_shim_factory(&mut self) {
        let Some(auth_token) = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        else {
            return;
        };
        let Some(transport) = self.transfer_runtime.network_transport() else {
            return;
        };
        let staging_root = self.config.paths.cache_dir.join("fuse-staging");

        // Apply mount config from the profile: `[mount].cache_size_mb`,
        // `[mount].page_cache_entries`, `[mount].metadata_ttl_secs`.
        let mount_cfg = &self.config.mount;
        let mut adapter_options = pcloud_fs::fuse_adapter::AdapterOptions::default();

        // Page-cache memory budget: config (MiB) is the baseline.
        let cache_bytes = (mount_cfg.cache_size_mb as u128) * 1024u128 * 1024u128;
        adapter_options.page_cache.max_bytes = cache_bytes.min(usize::MAX as u128 / 4) as usize;

        // Metadata-cache capacity from config.
        adapter_options.cache.capacity = mount_cfg.page_cache_entries as usize;

        // Metadata-cache TTL from config.
        adapter_options.cache.ttl =
            std::time::Duration::from_secs(mount_cfg.metadata_ttl_secs as u64);

        // Honour `PCLOUD_CACHE_SIZE_GB` exported by `pcloudc start` so
        // the user-config cache cap reaches the page cache. The CLI
        // owns the value (TOML); the daemon just reads the env on
        // mount-factory construction so a future `pcloudc start` with
        // a different value takes effect on next bind.
        // This env var takes precedence over the config file value.
        if let Some(gb) = std::env::var("PCLOUD_CACHE_SIZE_GB")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|n| *n > 0)
        {
            // Cap at usize::MAX/4 to avoid pathological overflow on
            // small-`usize` targets; for practical 64-bit hosts this
            // is unreachable.
            let bytes = (gb as u128) * 1024u128 * 1024u128 * 1024u128;
            adapter_options.page_cache.max_bytes = bytes.min(usize::MAX as u128 / 4) as usize;
        }

        let params = crate::mount_runtime::ShimFactoryParams {
            transport,
            auth_token,
            staging_root,
            write_options: pcloud_fs::write_path::WritePathOptions::default(),
            adapter_options,
        };
        let (factory, drain) = crate::mount_runtime::pcloud_shim_adapter_factory(params);
        self.mount_control.replace_factory(factory, drain);
    }

    /// Unmount the active filesystem mount. Triggers the drain hook first.
    /// Unmount the active pCloud FUSE mount, running the drain hook
    /// first.
    pub fn unmount_filesystem(&mut self) -> Response {
        self.mount_control.unmount()
    }

    /// Mirrors C `psync_create_remote_folder`,
    /// `psync_create_remote_folder_by_path`, and
    /// `psync_check_and_create_folder` (`pclsync/psynclib.c:1006,1020` and
    /// `pclsync/pbusinessaccount.c:803`). The variant is selected by the
    /// IPC payload:
    /// - `parent_folder_id = Some(id)` + `name` + `check_and_create=false` ->
    ///   bare `psync_create_remote_folder`,
    /// - `parent_folder_id = Some(id)` + `name` + `check_and_create=true` ->
    ///   `psync_check_and_create_folder` (suffix-retry helper),
    /// - `parent_folder_id = None` + `path` + `check_and_create=false` ->
    ///   `psync_create_remote_folder_by_path`.
    pub fn create_remote_folder(
        &mut self,
        parent_folder_id: Option<u64>,
        name: String,
        path: String,
        check_and_create: bool,
    ) -> Response {
        if parent_folder_id.is_some() {
            if name.trim().is_empty() {
                return Response {
                    status: ResponseStatus::InvalidRequest,
                    message: "create-remote-folder: 'name' must not be empty when parent_folder_id is set".to_owned(),
                };
            }
        } else if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "create-remote-folder: either parent_folder_id+name or absolute path is required".to_owned(),
            };
        } else if !path.starts_with('/') {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: format!(
                    "create-remote-folder: path must be absolute (start with '/'): {path:?}"
                ),
            };
        }

        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "create-remote-folder requires an authenticated session".to_owned(),
                };
            }
        };

        let (result, category, details): (
            Result<
                pcloud_proto::CreateFolderResponse,
                pcloud_proto::FolderApiError<FolderBackendError>,
            >,
            &'static str,
            String,
        ) = if let Some(parent) = parent_folder_id {
            if check_and_create {
                match self
                    .folder_runtime
                    .check_and_create_folder(auth_token, parent, name.clone())
                {
                    Ok((response, suffix)) => {
                        return self.audited_response(
                            "folder.check_and_create",
                            Some(format!("parent={parent} suffix={suffix}")),
                            format!(
                                "remote folder ready: parent={parent}, folder_id={}, suffix={suffix}, created={}",
                                response.folder_id, response.created
                            ),
                        );
                    }
                    Err(err) => (Err(err), "check_and_create", format!("parent={parent}")),
                }
            } else {
                (
                    self.folder_runtime
                        .create_remote_folder(auth_token, parent, name.clone()),
                    "create_by_parent",
                    format!("parent={parent}"),
                )
            }
        } else {
            (
                self.folder_runtime
                    .create_remote_folder_by_path(auth_token, path.clone()),
                "create_by_path",
                format!("path={path}"),
            )
        };

        match result {
            Ok(response) => self.audited_response(
                format!("folder.{category}"),
                Some(details),
                format!(
                    "remote folder created: folder_id={}, name={:?}",
                    response.folder_id, response.name
                ),
            ),
            Err(err) => map_folder_error(err),
        }
    }

    /// Verify the tamper-evident audit chain. If the daemon has been
    /// provisioned with an `PCLOUD_AUDIT_HMAC_KEY`, the HMAC is also
    /// cross-checked (audit non-repudiation).
    /// Verify the audit log hash chain over `range` and return the
    /// verification result (ok / mismatch / missing) as a response.
    ///
    /// # SLO wiring (I15 hot-path call site #7)
    ///
    /// Every verification outcome feeds
    /// `audit.hash_chain.verify.daily_pass_rate`. A verifier `Err`
    /// returned by `pcloud_store::verify_audit_chain` counts as a
    /// failed chain verification. Success paths count as a pass.
    pub fn audit_verify_chain(&self, range: pcloud_ipc::AuditVerifyRange) -> Response {
        let key = std::env::var_os("PCLOUD_AUDIT_HMAC_KEY")
            .map(|raw| raw.to_string_lossy().into_owned().into_bytes());
        let result =
            pcloud_store::verify_audit_chain(&self.store.db_path, range.from, range.to, key);
        let pass = result.is_ok();
        self.observability.slo.observe_audit_verify(pass);
        match result {
            Ok(v) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "audit: chain verified (entries={}, first_id={:?}, last_id={:?})",
                    v.entries_checked, v.first_id, v.last_id
                ),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("audit: chain verification FAILED: {err}"),
            },
        }
    }

    /// Mirrors `psync_get_{bool,int,uint,string}_value` reads from the C
    /// `setting` table. Missing rows are reported the same way the C helpers
    /// report them (`0` / `false` / empty string).
    /// Read a value-KV entry by `name` of the given `kind`, returning
    /// its serialized payload as a response.
    pub fn value_get(&self, name: &str, kind: pcloud_ipc::ValueKvKind) -> Response {
        let db_path = &self.store.db_path;
        let result = match kind {
            pcloud_ipc::ValueKvKind::Bool => pcloud_store::value_kv::get_bool(db_path, name)
                .map(|value| format!("value:bool={value}")),
            pcloud_ipc::ValueKvKind::Int => pcloud_store::value_kv::get_int(db_path, name)
                .map(|value| format!("value:int={value}")),
            pcloud_ipc::ValueKvKind::Uint => pcloud_store::value_kv::get_uint(db_path, name)
                .map(|value| format!("value:uint={value}")),
            pcloud_ipc::ValueKvKind::String => pcloud_store::value_kv::get_string(db_path, name)
                .map(|value| match value {
                    Some(text) => format!("value:string={text}"),
                    None => "value:string=".to_owned(),
                }),
        };
        match result {
            Ok(message) => Response {
                status: ResponseStatus::Ok,
                message,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("value_kv read failed: {err}"),
            },
        }
    }

    /// Mirrors `psync_set_{bool,int,uint,string}_value`.
    /// Upsert a value-KV entry named `name` with the given typed
    /// payload. Returns an Ok response on success.
    pub fn value_set(&self, name: &str, value: pcloud_ipc::ValueKvPayload) -> Response {
        let db_path = &self.store.db_path;
        let result = match value {
            pcloud_ipc::ValueKvPayload::Bool(value) => {
                pcloud_store::value_kv::set_bool(db_path, name, value)
            }
            pcloud_ipc::ValueKvPayload::Int(value) => {
                pcloud_store::value_kv::set_int(db_path, name, value)
            }
            pcloud_ipc::ValueKvPayload::Uint(value) => {
                pcloud_store::value_kv::set_uint(db_path, name, value)
            }
            pcloud_ipc::ValueKvPayload::String(value) => {
                pcloud_store::value_kv::set_string(db_path, name, &value)
            }
        };
        match result {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: "value_kv: ok".to_owned(),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("value_kv write failed: {err}"),
            },
        }
    }

    /// Strict presence+kind check. No direct C analogue; the C API relies on
    /// non-zero reads to emulate presence.
    /// Return whether a value-KV entry named `name` of `kind` exists.
    pub fn value_has(&self, name: &str, kind: pcloud_ipc::ValueKvKind) -> Response {
        let db_path = &self.store.db_path;
        let result = match kind {
            pcloud_ipc::ValueKvKind::Bool => pcloud_store::value_kv::has_bool(db_path, name),
            pcloud_ipc::ValueKvKind::Int => pcloud_store::value_kv::has_int(db_path, name),
            pcloud_ipc::ValueKvKind::Uint => pcloud_store::value_kv::has_uint(db_path, name),
            pcloud_ipc::ValueKvKind::String => pcloud_store::value_kv::has_string(db_path, name),
        };
        match result {
            Ok(present) => Response {
                status: ResponseStatus::Ok,
                message: format!("value:has={present}"),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("value_kv has failed: {err}"),
            },
        }
    }

    /// Fetch the current account notifications list. Mirrors C
    /// `psync_get_notifications` (pclsync/psynclib.c:248). Unlike the C
    /// client, which serves the list from the long-poll diff cache, the Rust
    /// runtime dispatches a dedicated `listnotifications` request through
    /// [`NotificationsRuntime`], keeping the active path self-contained.
    /// Fetch the authenticated user's notifications via the
    /// notifications runtime and return them as a JSON response.
    pub fn list_notifications(&mut self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "notification listing requires an authenticated session".to_owned(),
                };
            }
        };

        match self
            .notifications_runtime
            .list_notifications(auth_token, None)
        {
            Ok(list) => {
                let rendered = if list.is_empty() {
                    "[]".to_owned()
                } else {
                    list.iter()
                        .map(|n| {
                            format!(
                                "{{id={}, read={}, created_at={}, text=\"{}\", thumb={}}}",
                                n.id,
                                n.read,
                                n.created_at,
                                n.text.replace('"', "\\\""),
                                n.thumbnail_url.as_deref().unwrap_or("")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.audited_response(
                    "notifications.list",
                    Some(format!("count={}", list.len())),
                    format!("notifications: count={}, {}", list.len(), rendered),
                )
            }
            Err(err) => map_notifications_error(err),
        }
    }

    /// Mark all account notifications up to and including `upto_id` as read.
    /// Mirrors C `psync_mark_notificaitons_read` (sic - pclsync/psynclib.c:324).
    /// Mark all notifications with id `<= upto_id` as read on the
    /// server.
    pub fn mark_notifications_read(&mut self, upto_id: u64) -> Response {
        if upto_id == 0 {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "notificationid must be non-zero".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "marking notifications read requires an authenticated session"
                        .to_owned(),
                };
            }
        };

        match self
            .notifications_runtime
            .mark_notifications_read(auth_token, upto_id)
        {
            Ok(()) => self.audited_response(
                "notifications.mark_read",
                Some(format!("upto_id={upto_id}")),
                format!("notifications marked read up to id={upto_id}"),
            ),
            Err(err) => map_notifications_error(err),
        }
    }

    /// Drive one pass of the remote-diff poller from `cursor` up to
    /// `limit` entries, returning the count of applied changes.
    pub fn poll_remote_diff_once(&mut self, cursor: u64, limit: u64) -> Result<usize, String> {
        let auth_token = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or_else(|| "remote diff requires an authenticated session".to_owned())?;
        let batch = self
            .sync_runtime
            .diff(auth_token, cursor, limit)
            .map_err(|err| err.to_string())?;
        let operations = self
            .engine
            .ingest_remote_diff(&batch)
            .map_err(|err| format!("failed to ingest diff batch: {err:?}"))?;
        Ok(operations.len())
    }

    /// Prepare queued downloads (reserve ids, build plan state).
    /// Returns the number of items prepared in this pass.
    pub fn prepare_active_downloads_once(&mut self) -> Result<usize, String> {
        let auth_token = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or_else(|| "download preparation requires an authenticated session".to_owned())?;
        let mut prepared = 0usize;
        for task in &self.engine.downloads.active_downloads {
            if let pcloud_model::sync::PlannedOperation::DownloadFile {
                remote_file_id: Some(file_id),
                ..
            } = &task.operation
            {
                self.transfer_runtime
                    .get_file_link(auth_token.clone_secret(), file_id.get(), None)
                    .map_err(|err| err.to_string())?;
                prepared += 1;
            }
        }
        Ok(prepared)
    }

    /// Prepare queued uploads (stage local data, reserve upload ids).
    /// Returns the number of items prepared in this pass.
    pub fn prepare_active_uploads_once(&mut self) -> Result<usize, String> {
        let auth_token = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or_else(|| "upload preparation requires an authenticated session".to_owned())?;
        let mut prepared = 0usize;
        for task in &self.engine.uploads.active_uploads {
            if let pcloud_model::sync::PlannedOperation::UploadFile {
                path,
                remote_parent_folder_id,
                remote_name,
                ..
            } = &task.operation
            {
                let parent_folder_id =
                    match resolve_upload_parent_folder_id(path, *remote_parent_folder_id) {
                        Ok(folder_id) => folder_id,
                        Err(_) => continue,
                    };
                self.transfer_runtime
                    .upload_create(
                        auth_token.clone_secret(),
                        parent_folder_id,
                        remote_name.clone(),
                        0,
                    )
                    .map_err(|err| err.to_string())?;
                prepared += 1;
            }
        }
        Ok(prepared)
    }

    /// Execute prepared downloads (signed HTTP fetch + disk write).
    /// Returns the number of items executed in this pass.
    pub fn execute_active_downloads_once(&mut self) -> Result<usize, String> {
        let auth_token = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or_else(|| "download execution requires an authenticated session".to_owned())?;
        let tasks = self.engine.downloads.active_downloads.clone();
        let mut completed = 0usize;

        for task in tasks {
            if let pcloud_model::sync::PlannedOperation::DownloadFile {
                path,
                remote_file_id: Some(file_id),
                ..
            } = &task.operation
            {
                match self.transfer_runtime.get_file_link(
                    auth_token.clone_secret(),
                    file_id.get(),
                    None,
                ) {
                    Ok(link) => match self.transfer_runtime.download_bytes(&link) {
                        Ok((signed, bytes)) => {
                            #[cfg(feature = "metrics")]
                            self.metric_add_transfer_bytes(
                                TransferDirection::Download,
                                bytes.len() as u64,
                            );
                            let cache_key = format!("download:{}:{}", signed.host, path);
                            self.cache.cache_page(cache_key, bytes.clone());
                            self.cache.stage_file(path.clone(), bytes.clone());
                            self.filesystem.seed_staged_file(path.clone(), bytes);
                            if self.engine.mark_transfer_completed(path) {
                                completed += 1;
                            }
                        }
                        Err(err) => {
                            let decision = self.engine.classify_failure(
                                &task.operation,
                                pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                            );
                            let message = format!("{err}; recovery={:?}", decision.disposition);
                            // Audit finding M1: do not swallow the audit
                            // signal when a transfer failure cannot be
                            // recorded. A `false` return indicates the
                            // transfer is not tracked, which is a diagnostic
                            // event worth surfacing on stderr.
                            if !self.engine.mark_transfer_failed(path, message) {
                                log::warn!(
                                    "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                                );
                            }
                        }
                    },
                    Err(err) => {
                        let decision = self.engine.classify_failure(
                            &task.operation,
                            pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                        );
                        let message = format!("{err}; recovery={:?}", decision.disposition);
                        // Audit finding M1: surface audit-dropped signal.
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                    }
                }
            }
        }

        Ok(completed)
    }

    /// Execute prepared uploads (upload-write + upload-save). Returns
    /// the number of items executed in this pass.
    pub fn execute_active_uploads_once(&mut self) -> Result<usize, String> {
        let auth_token = self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
            .ok_or_else(|| "upload execution requires an authenticated session".to_owned())?;
        let tasks = self.engine.uploads.active_uploads.clone();
        let mut completed = 0usize;

        for task in tasks {
            if let pcloud_model::sync::PlannedOperation::UploadFile {
                path,
                remote_parent_folder_id,
                remote_name,
                ..
            } = &task.operation
            {
                let payload = match self.read_local_upload_payload(path) {
                    Ok(payload) => payload,
                    Err(failure) => {
                        let decision = self.engine.classify_failure(&task.operation, failure);
                        let message = format!(
                            "missing staged upload payload; recovery={:?}",
                            decision.disposition
                        );
                        // Audit finding M1: surface audit-dropped signal.
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                        continue;
                    }
                };
                let parent_folder_id = match resolve_upload_parent_folder_id(
                    path,
                    *remote_parent_folder_id,
                ) {
                    Ok(folder_id) => folder_id,
                    Err(failure) => {
                        let decision = self.engine.classify_failure(&task.operation, failure);
                        let message = format!(
                            "missing upload destination metadata; recovery={:?}",
                            decision.disposition
                        );
                        // Audit finding M1: do not swallow the audit
                        // signal when a transfer failure cannot be
                        // recorded. A `false` return indicates the
                        // transfer is not tracked, which is a diagnostic
                        // event worth surfacing on stderr.
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                        continue;
                    }
                };
                match self.transfer_runtime.upload_create(
                    auth_token.clone_secret(),
                    parent_folder_id,
                    remote_name.clone(),
                    payload.len() as u64,
                ) {
                    Ok(session) => {
                        // SLO wiring (I15 hot-path call site #4).
                        // Time the actual wire upload and compute
                        // throughput in MB/s at completion; feeds
                        // `upload.throughput_mbps.p50`. We deliberately
                        // measure only the `upload_bytes` call (write +
                        // save) so the SLI reflects effective upload
                        // throughput and is not diluted by session
                        // creation RTT.
                        let upload_started = std::time::Instant::now();
                        match self.transfer_runtime.upload_bytes(
                            auth_token.clone_secret(),
                            &session,
                            &payload,
                        ) {
                            Ok(_frame) => {
                                let elapsed = upload_started.elapsed();
                                let bytes = payload.len() as u64;
                                let secs = elapsed.as_secs_f64();
                                if secs > 0.0 && bytes > 0 {
                                    // MB/s = bytes / (10^6 * seconds)
                                    let mbps = (bytes as f64) / 1.0e6 / secs;
                                    self.observability.slo.observe_upload_throughput_mbps(mbps);
                                }
                                #[cfg(feature = "metrics")]
                                self.metric_add_transfer_bytes(
                                    TransferDirection::Upload,
                                    payload.len() as u64,
                                );
                                let cache_key = format!("upload:{}:{}", session.upload_id, path);
                                self.cache.cache_page(cache_key, payload);
                                if self.engine.mark_transfer_completed(path) {
                                    completed += 1;
                                }
                            }
                            Err(err) => {
                                let decision = self.engine.classify_failure(
                                    &task.operation,
                                    pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                                );
                                let message = format!("{err}; recovery={:?}", decision.disposition);
                                // Audit finding M1: do not swallow the audit
                                // signal when a transfer failure cannot be
                                // recorded. A `false` return indicates the
                                // transfer is not tracked, which is a diagnostic
                                // event worth surfacing on stderr.
                                if !self.engine.mark_transfer_failed(path, message) {
                                    log::warn!(
                                        "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                                    );
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let decision = self.engine.classify_failure(
                            &task.operation,
                            pcloud_engine::recovery::RecoveryFailure::RetryableNetworkError,
                        );
                        let message = format!("{err}; recovery={:?}", decision.disposition);
                        // Audit finding M1: surface audit-dropped signal.
                        if !self.engine.mark_transfer_failed(path, message) {
                            log::warn!(
                                "audit: mark_transfer_failed dropped for untracked transfer path={path:?}"
                            );
                        }
                    }
                }
            }
        }

        Ok(completed)
    }

    /// Update the active API server endpoint in the config and the
    /// live transports. Rejects downgrades in production per the
    /// transport policy rules.
    pub fn set_api_server(
        &mut self,
        binapi: impl Into<String>,
        location_id: u32,
    ) -> Result<(), pcloud_store::StoreError> {
        let binapi = binapi.into();
        if let Err(reason) = self.config.api.apply_api_server_hint(&binapi) {
            log::error!(
                "set_api_server: rejecting binapi hint '{}' — {}",
                binapi,
                reason
            );
        }
        self.auth_runtime.apply_api_server_hint(&binapi);
        self.account_runtime.apply_api_server_hint(&binapi);
        self.backup_runtime.apply_api_server_hint(&binapi);
        self.sync_runtime.apply_api_server_hint(&binapi);
        self.transfer_runtime.apply_api_server_hint(&binapi);
        self.public_link_runtime.apply_api_server_hint(&binapi);
        self.store.repositories.preferences.api_server_binapi = Some(binapi);
        self.store.repositories.preferences.api_server_location_id = Some(location_id);
        persist_profile(&self.store)
    }

    /// `GetApiServers` IPC handler — return the list of pCloud API regions.
    /// No auth required. Mirrors C `psync_get_api_servers`.
    fn get_api_servers(&self) -> Response {
        match self.account_runtime.get_api_servers() {
            Ok(servers) => {
                let json = serde_json::json!(
                    servers
                        .iter()
                        .map(|s| serde_json::json!({
                            "label": s.label,
                            "api": s.api,
                            "binapi": s.binapi,
                            "location_id": s.location_id,
                        }))
                        .collect::<Vec<_>>()
                );
                Response {
                    status: ResponseStatus::Ok,
                    message: json.to_string(),
                }
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("get_api_servers failed: {err}"),
            },
        }
    }

    /// `GetPromo` IPC handler — fetch promotional URL for this platform.
    /// Requires an authenticated session. Mirrors C `psync_get_promo`.
    fn get_promo(&self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "get_promo requires an authenticated session".to_owned(),
                };
            }
        };
        match self.account_runtime.get_promo(auth_token) {
            Ok(Some(promo)) => Response {
                status: ResponseStatus::Ok,
                message: serde_json::json!({
                    "url": promo.url,
                    "width": promo.width,
                    "height": promo.height,
                })
                .to_string(),
            },
            Ok(None) => Response {
                status: ResponseStatus::Ok,
                message: "no promo".to_owned(),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("get_promo failed: {err}"),
            },
        }
    }

    /// `GetCryptoHint` IPC handler — return the stored passphrase hint.
    /// Mirrors C `psync_crypto_hint`.
    fn get_crypto_hint(&self) -> Response {
        match self.crypto.get_hint() {
            Some(hint) if !hint.is_empty() => Response {
                status: ResponseStatus::Ok,
                message: hint.to_owned(),
            },
            _ => {
                if self.crypto.is_setup() {
                    Response {
                        status: ResponseStatus::Ok,
                        message: String::new(),
                    }
                } else {
                    Response {
                        status: ResponseStatus::Unavailable,
                        message: "crypto is not set up; no hint available".to_owned(),
                    }
                }
            }
        }
    }

    /// `VerifyEmail` IPC handler — trigger a server-side verification email.
    /// Requires an authenticated session. Mirrors C `psync_verify_email`.
    fn verify_email(&self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "verify_email requires an authenticated session".to_owned(),
                };
            }
        };
        match self.account_runtime.verify_email(auth_token) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: "verification email sent".to_owned(),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("verify_email failed: {err}"),
            },
        }
    }

    /// `LostPassword` IPC handler — send a password-reset email.
    /// No auth required. Mirrors C `psync_lost_password`.
    fn lost_password(&self, email: String) -> Response {
        if email.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "lost_password: email must not be empty".to_owned(),
            };
        }
        match self.account_runtime.lost_password(&email) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!("password reset email sent to {email}"),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("lost_password failed: {err}"),
            },
        }
    }

    /// `VerifyEmailRestricted` IPC handler — verify email with a token.
    /// Mirrors C `psync_verify_email_restricted`.
    fn verify_email_restricted(&self, verify_token: String) -> Response {
        if verify_token.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "verify_email_restricted: verify_token must not be empty".to_owned(),
            };
        }
        match self.account_runtime.verify_email_restricted(&verify_token) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: "email verified successfully".to_owned(),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("verify_email_restricted failed: {err}"),
            },
        }
    }

    /// `AccountChangePassword` IPC handler — change the account password.
    /// Requires an authenticated session. Mirrors C `psync_change_password`.
    fn account_change_password(
        &mut self,
        current_password: SecretString,
        new_password: SecretString,
    ) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "change_password requires an authenticated session".to_owned(),
                };
            }
        };
        if current_password.is_empty() || new_password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "current and new passwords must not be empty".to_owned(),
            };
        }
        match self.account_runtime.change_password(
            auth_token,
            current_password.expose_secret(),
            new_password.expose_secret(),
            "pcloudc",
        ) {
            Ok(result) => {
                // Install the new auth token returned by the server.
                let user_id = self.auth.snapshot().authenticated_user;
                let new_token = SecretString::new(result.auth_token);
                if let Err(err) = self
                    .auth
                    .apply(pcloud_auth::AuthCommand::MarkAuthenticated {
                        user_id,
                        auth_token: new_token,
                    })
                {
                    return Response {
                        status: ResponseStatus::InternalError,
                        message: format!("password changed but token update failed: {err}"),
                    };
                }
                self.audited_response(
                    "account.change_password",
                    None,
                    "account password changed successfully",
                )
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("change_password failed: {err}"),
            },
        }
    }

    /// `AccountRegister` IPC handler — register a new pCloud account.
    /// No auth required. Mirrors C `psync_register`.
    fn account_register(
        &self,
        email: String,
        password: SecretString,
        terms_accepted: bool,
    ) -> Response {
        if email.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "register: email must not be empty".to_owned(),
            };
        }
        if password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "register: password must not be empty".to_owned(),
            };
        }
        if !terms_accepted {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "register: terms of service must be accepted (pass --accept-terms)"
                    .to_owned(),
            };
        }
        match self
            .account_runtime
            .register(&email, password, terms_accepted, 3)
        {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!("account registered: {email}"),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("register failed: {err}"),
            },
        }
    }

    /// `GetFileLink` IPC handler — resolve a download URL for a file id.
    /// Requires an authenticated session. Mirrors C `psync_get_file_link`.
    fn get_file_link_ipc(&self, file_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "get_file_link requires an authenticated session".to_owned(),
                };
            }
        };
        match self
            .transfer_runtime
            .get_file_link(auth_token, file_id, None)
        {
            Ok(link) => {
                let json = serde_json::json!({
                    "hosts": link.hosts,
                    "path": link.path,
                    "download_tag": link.download_tag,
                });
                Response {
                    status: ResponseStatus::Ok,
                    message: json.to_string(),
                }
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("get_file_link failed: {err}"),
            },
        }
    }

    /// `DownloadFile` IPC handler — download a remote file to a local path.
    /// Requires an authenticated session.
    fn download_file_ipc(&self, file_id: u64, local_path: std::path::PathBuf) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "download_file requires an authenticated session".to_owned(),
                };
            }
        };
        let link = match self
            .transfer_runtime
            .get_file_link(auth_token, file_id, None)
        {
            Ok(link) => link,
            Err(err) => {
                return Response {
                    status: ResponseStatus::InternalError,
                    message: format!("get_file_link failed: {err}"),
                };
            }
        };
        let (_signed, bytes) = match self.transfer_runtime.download_bytes(&link) {
            Ok(result) => result,
            Err(err) => {
                return Response {
                    status: ResponseStatus::InternalError,
                    message: format!("download_bytes failed: {err}"),
                };
            }
        };
        match std::fs::write(&local_path, &bytes) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "downloaded {} bytes to {}",
                    bytes.len(),
                    local_path.display()
                ),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("write to {} failed: {err}", local_path.display()),
            },
        }
    }

    /// `DeleteBackup` IPC handler — delete a backup by folder id.
    /// Requires an authenticated session. Mirrors C `psync_delete_backup`.
    fn delete_backup_ipc(&mut self, backup_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "delete_backup requires an authenticated session".to_owned(),
                };
            }
        };
        // Call stop_backup on the server side. The cascade (sync-root removal)
        // is intentionally skipped here as the daemon's sync roots are managed
        // through the IPC surface separately. Documented scope: the user removes
        // the associated sync root via `pcloudc sync remove` independently.
        match self.backup_runtime.stop_backup(auth_token, backup_id) {
            Ok(()) => self.audited_response(
                "backup.delete",
                Some(format!("backup_id={backup_id}")),
                format!("backup {backup_id} deleted"),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("delete_backup failed: {err}"),
            },
        }
    }

    /// `CreateBackup` IPC handler — create a backup on the backend and
    /// persist the resulting device-root folder id so later `StopDevice` /
    /// `DeleteBackupDevice` calls can locate it. Mirrors C
    /// `psync_create_backup`. Requires an authenticated session.
    ///
    /// Side effects on success:
    /// * `preferences.backup_device_folder_id` is set to the parent folder
    ///   id reported by the backend (falling back to the backup folder id
    ///   itself when the backend omits `parentfolderid`), mirroring the
    ///   SDK's `set_backup_device_folder_id` contract.
    /// * The response `message` includes `device_folder_id=<N> folder_id=<M>`
    ///   as an unambiguous key=value line so callers (and the live-e2e
    ///   test harness) can extract the device folder id deterministically.
    ///
    /// The `local_path` is validated for non-emptiness but this handler
    /// intentionally does NOT auto-register a sync root — that cascade
    /// remains an explicit `Request::SyncRootAdd` call so the IPC surface
    /// stays orthogonal and the daemon never silently mutates sync-root
    /// state behind the operator's back.
    fn create_backup_ipc(
        &mut self,
        name: String,
        root_folder_id: u64,
        local_path: String,
        parent_folder_name: Option<String>,
    ) -> Response {
        if name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "create_backup: name must not be empty".to_owned(),
            };
        }
        if local_path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "create_backup: local_path must not be empty".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "create_backup requires an authenticated session".to_owned(),
                };
            }
        };
        match self.backup_runtime.create_backup(
            auth_token,
            name.clone(),
            root_folder_id,
            parent_folder_name,
        ) {
            Ok(created) => {
                let device_folder_id = created.parent_folder_id.unwrap_or(created.folder_id);
                self.store.repositories.preferences.backup_device_folder_id =
                    Some(device_folder_id);
                if let Err(err) = persist_profile(&self.store) {
                    // Persistence failed: surface clearly, but the backend
                    // already created the backup. Report both facts.
                    return Response {
                        status: ResponseStatus::InternalError,
                        message: format!(
                            "create_backup succeeded (folder_id={} device_folder_id={}) but persisting device folder id failed: {err}",
                            created.folder_id, device_folder_id,
                        ),
                    };
                }
                self.audited_response(
                    "backup.create",
                    Some(format!(
                        "name={name} folder_id={} device_folder_id={} root_folder_id={root_folder_id}",
                        created.folder_id, device_folder_id,
                    )),
                    format!(
                        "backup created: device_folder_id={} folder_id={} name={:?}",
                        device_folder_id, created.folder_id, name,
                    ),
                )
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("create_backup failed: {err}"),
            },
        }
    }

    /// `StopDevice` IPC handler — stop a device backup by its device
    /// folder id. Mirrors C `psync_stop_device`. Requires an authenticated
    /// session.
    fn stop_device_ipc(&mut self, device_folder_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "stop_device requires an authenticated session".to_owned(),
                };
            }
        };
        match self
            .backup_runtime
            .stop_device(auth_token, device_folder_id)
        {
            Ok(()) => self.audited_response(
                "backup.stop_device",
                Some(format!("device_folder_id={device_folder_id}")),
                format!("device stopped: device_folder_id={device_folder_id}"),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("stop_device failed: {err}"),
            },
        }
    }

    /// `DeleteBackupDevice` IPC handler — clear the locally persisted
    /// device backup-root folder id. Mirrors C
    /// `psync_delete_backup_device`. Local-only: does not talk to the
    /// backend.
    fn delete_backup_device_ipc(&mut self) -> Response {
        let previous = self.store.repositories.preferences.backup_device_folder_id;
        self.store.repositories.preferences.backup_device_folder_id = None;
        match persist_profile(&self.store) {
            Ok(()) => self.audited_response(
                "backup.delete_device",
                Some(format!(
                    "previous_device_folder_id={}",
                    previous
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "none".to_owned()),
                )),
                format!(
                    "local backup device cleared (previous_device_folder_id={})",
                    previous
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "none".to_owned()),
                ),
            ),
            Err(err) => {
                // Roll back so in-memory state matches on-disk state.
                self.store.repositories.preferences.backup_device_folder_id = previous;
                Response {
                    status: ResponseStatus::InternalError,
                    message: format!("delete_backup_device: persist failed: {err}"),
                }
            }
        }
    }

    /// `SetApiServer` IPC handler — pin the daemon to a specific API region.
    /// Mirrors C `psync_set_api_server`.
    fn set_api_server_ipc(&mut self, location_id: u32, binapi: String) -> Response {
        if binapi.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "set_api_server: binapi must not be empty".to_owned(),
            };
        }
        match self.set_api_server(binapi.clone(), location_id) {
            Ok(()) => self.audited_response(
                "account.set_api_server",
                Some(format!("location_id={location_id} binapi={binapi}")),
                format!("API server set to {binapi} (location_id={location_id})"),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("set_api_server failed: {err}"),
            },
        }
    }

    /// `SetLanguage` IPC handler — set the account language preference.
    /// Requires an authenticated session. Mirrors C `psync_set_language`.
    fn set_language_ipc(&self, language: String) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "set_language requires an authenticated session".to_owned(),
                };
            }
        };
        if language.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "set_language: language must not be empty".to_owned(),
            };
        }
        match self.account_runtime.set_language(auth_token, &language) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!("language set to {language}"),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("set_language failed: {err}"),
            },
        }
    }

    /// `UploadWriteFromFile` IPC handler — server-side copy.
    ///
    /// The C `upload_writefromfile` primitive copies bytes from a remote
    /// pCloud `(fileid, hash)` source into an in-progress upload session
    /// (`pcloud_proto::methods::upload::UploadWriteFromFileRequest`,
    /// params: `uploadid` / `fileid` / `hash` / `uploadoffset` /
    /// `offset` / `count` — cited: `pclsync/pupload.c:843-859`).
    ///
    /// audit-06 H-4.2 + bd-1du row 93: routes the IPC call to
    /// `TransferRuntime::upload_write_from_file`, which encodes the
    /// frame, executes it on the live `BinaryApiTransport`, and
    /// classifies the server result code.
    fn upload_write_from_file_ipc(
        &mut self,
        upload_session_id: u64,
        source_fileid: u64,
        source_hash: u64,
        offset: u64,
        count: u64,
    ) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "upload_writefromfile requires an authenticated session".to_owned(),
                };
            }
        };
        // Stable correlation id derived from the destination offset —
        // the server only requires uniqueness within a single upload
        // session, and offsets are guaranteed monotone per chunk.
        let chunk_id = offset / (pcloud_proto::transfer_api::PSYNC_COPY_BUFFER_SIZE as u64);
        match self.transfer_runtime.upload_write_from_file(
            auth_token,
            upload_session_id,
            offset,
            chunk_id,
            source_fileid,
            source_hash,
            // Source offset mirrors the upload offset for the simple
            // 1:1 copy carried by this IPC variant. The full C calling
            // convention allows independent values; the IPC variant
            // carries only one `offset` field today (matching the
            // common case in the upstream code).
            offset,
            count,
        ) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "upload_writefromfile ok: {count} bytes from fileid {source_fileid} into upload {upload_session_id} at offset {offset}"
                ),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("upload_writefromfile failed: {err}"),
            },
        }
    }

    /// `CreateTreePublicLinkFromPaths` IPC handler — resolve a list of
    /// absolute pCloud-drive paths to their remote folder ids on the
    /// daemon side, then create a tree public link.  Mirrors the C
    /// `ptree_public_link` path-based variant (row 149, bd-1du).
    fn create_tree_public_link_from_paths_ipc(
        &mut self,
        name: String,
        paths: Vec<String>,
        expires: Option<u64>,
    ) -> Response {
        if name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "create_tree_public_link_from_paths: name must not be empty".to_owned(),
            };
        }
        if paths.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "create_tree_public_link_from_paths: at least one path is required"
                    .to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "create_tree_public_link_from_paths requires an authenticated session"
                        .to_owned(),
                };
            }
        };

        // Resolve each path to a remote folder id using the path resolver.
        let resolver = self
            .public_link_runtime
            .path_resolver(auth_token.clone_secret());
        let mut folder_ids: Vec<u64> = Vec::with_capacity(paths.len());
        for path in &paths {
            match resolver.get_folder_id_by_path(path) {
                Ok(id) => folder_ids.push(id.get()),
                Err(err) => {
                    return map_path_resolve_error(err);
                }
            }
        }
        let folder_ids_csv = folder_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        match self.public_link_runtime.create_tree_public_link(
            auth_token,
            name.clone(),
            None,
            Some(folder_ids_csv.clone()),
            None,
            expires,
            None,
            None,
        ) {
            Ok(created) => self.audited_response(
                "publinks.create_tree_from_paths",
                Some(format!(
                    "name={name} paths={paths:?} resolved_folder_ids={folder_ids_csv}",
                )),
                format!(
                    "tree public link created from paths: id={}, name=\"{}\", link=\"{}\"",
                    created.link_id, created.name, created.link,
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn pause_sync(&mut self) -> Response {
        self.engine.sync_state = SyncState::Paused;
        if let Some(shared) = &self.sync_loop_shared {
            shared.pause();
        }
        self.audited_response(
            "sync.pause",
            None,
            format!("sync state set to {:?}", self.engine.sync_state),
        )
    }

    fn resume_sync(&mut self) -> Response {
        self.engine.sync_state = SyncState::Steady;
        if let Some(shared) = &self.sync_loop_shared {
            shared.resume();
        }
        self.audited_response(
            "sync.resume",
            None,
            format!("sync state set to {:?}", self.engine.sync_state),
        )
    }

    fn lock_crypto(&mut self) -> Response {
        self.crypto.lock();
        self.metric_sync_crypto_state();
        self.audited_response(
            "crypto.lock",
            None,
            format!("crypto state set to {:?}", self.crypto.unlock_state),
        )
    }

    fn request_shutdown(&mut self) -> Response {
        self.control.shutdown_requested = true;
        self.audited_response("daemon.shutdown_requested", None, "shutdown requested=true")
    }

    fn logout(&mut self) -> Response {
        // Tear down side-effects that depend on an authenticated
        // session BEFORE we drop the auth token. In order:
        //   1. Active FUSE mount — its read/write paths embed the auth
        //      token (page-cache backend, write-path uploader). Leaving
        //      the mount live after logout would let the kernel keep
        //      hitting the daemon with requests it can no longer fulfil
        //      and would be confusing UX-wise. Drain pending writes.
        //   2. Crypto state — locked via the existing crypto runtime so
        //      session-bound key material is wiped.
        //   3. Auth token + session — cleared via `AuthCommand::Logout`
        //      which also nulls the on-disk vault when `authsave` is
        //      enabled (sync_auth_vault detects the empty token).
        let mount_outcome = if self.mount_control.is_mounted() {
            Some(self.mount_control.unmount())
        } else {
            None
        };
        if self.crypto.is_started() {
            self.crypto.stop();
            self.metric_sync_crypto_state();
        }
        let auth_response = match self.auth.apply(AuthCommand::Logout) {
            Ok(event) => self.auth_response(event),
            Err(err) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: err.to_string(),
                };
            }
        };
        // Surface the mount-unmount outcome alongside the auth response
        // so the operator sees both happened atomically.
        match mount_outcome {
            Some(m) => Response {
                status: auth_response.status,
                message: format!("{} | {}", auth_response.message, m.message),
            },
            None => auth_response,
        }
    }

    fn unlock_crypto(&mut self, password: SecretString) -> Response {
        if password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto password must not be empty".to_owned(),
            };
        }
        let secret = password;
        let result = if self.crypto.is_setup() {
            self.crypto.start(secret)
        } else {
            self.crypto.unlock(secret)
        };
        self.metric_sync_crypto_state();
        match result {
            Ok(()) => self.audited_response(
                "crypto.start",
                Some(format!("state={:?}", self.crypto.unlock_state)),
                format!(
                    "crypto started (backend={}, setup={}, folders={})",
                    self.crypto.effective_backend(),
                    self.crypto.is_setup(),
                    self.crypto.folders.len()
                ),
            ),
            Err(pcloud_crypto::CryptoError::WrongPassword) => Response {
                status: ResponseStatus::Unauthorized,
                message: "wrong crypto password".to_owned(),
            },
            Err(err) => Response {
                status: ResponseStatus::Conflict,
                message: err.to_string(),
            },
        }
    }

    fn setup_crypto(&mut self, password: SecretString, hint: Option<String>) -> Response {
        if password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto password must not be empty".to_owned(),
            };
        }
        let result = self.crypto.setup(password, hint);
        self.metric_sync_crypto_state();
        match result {
            Ok(()) => self.audited_response(
                "crypto.setup",
                None,
                format!(
                    "crypto setup complete (state={:?})",
                    self.crypto.unlock_state
                ),
            ),
            Err(err) => Response {
                status: ResponseStatus::Conflict,
                message: err.to_string(),
            },
        }
    }

    fn crypto_mkdir(
        &mut self,
        name: String,
        parent_folder_id: Option<u64>,
        local_folder_id: Option<u64>,
    ) -> Response {
        if name.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "encrypted folder name must not be empty".to_owned(),
            };
        }
        // Stage 4b.3: migrated to `mkdir_with_context` so the
        // PclsyncCompat backend can surface the freshly-generated
        // `SymKeyVer1`; we then cache it locally against the new folder
        // id so subsequent filename / sector ops under this folder
        // resolve without another `crypto_getfolderkey` round-trip.
        // Enhanced shells receive `sym_key = None` and the legacy shape
        // is preserved byte-for-byte.
        let created = match self
            .crypto
            .mkdir_with_context(parent_folder_id, &name, local_folder_id)
        {
            Ok(c) => c,
            Err(pcloud_crypto::CryptoError::Locked) => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "crypto is locked".to_owned(),
                };
            }
            Err(err) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: err.to_string(),
                };
            }
        };
        let entry = created.entry.clone();
        if let Some(sym) = created.sym_key {
            // PclsyncCompat only. Insert the fresh folder sym-key into
            // the runtime cache. Server-side wrap + upload happens under
            // a separate `crypto_createfolder` call that is out of this
            // bead's scope (TODO(bd-1du.10)); what we need here is local
            // coherence so downstream filename encoding does not fault
            // with `FolderKeyNotCached`.
            if let Err(err) = self.crypto.cache_folder_key(entry.folder_id, sym) {
                log::warn!(
                    "crypto.mkdir: cache_folder_key failed (folder_id={}): {err}",
                    entry.folder_id,
                );
            }
        }
        self.audited_response(
            "crypto.mkdir",
            Some(format!("folder_id={}", entry.folder_id)),
            format!(
                "crypto folder created: id={}, encrypted_name_len={}",
                entry.folder_id,
                entry.encrypted_name.len()
            ),
        )
    }

    // ------------------------------------------------------------------
    // Stage 4b.3 — CryptoSetupV2 / CryptoGetFolderKey / CryptoGetFileKey
    // ------------------------------------------------------------------

    /// Dispatch [`Request::CryptoSetupV2`]. Translates
    /// [`CryptoBackendIpc`] manually (no From/Into by contract), enforces
    /// the `acknowledge_not_interop` gate on the Enhanced backend, then
    /// routes:
    ///
    /// - `PclsyncCompat`: runs `CryptoShell::setup_with_backend` locally
    ///   to produce sealed `priv_key_ver1` / `pub_key_ver1` blobs, then
    ///   uploads them via `crypto_setuserkeys`.
    /// - `Enhanced`: runs `CryptoShell::setup_with_backend` locally; no
    ///   server round-trip (the Enhanced profile is by design not
    ///   interoperable with the pcloudcom server-side setup record).
    ///
    /// Audit log entry is emitted before the response is returned so the
    /// operator can observe the backend choice even if the response is
    /// dropped on the wire.
    fn setup_crypto_v2(
        &mut self,
        backend: CryptoBackendIpc,
        acknowledge_not_interop: bool,
        password: SecretString,
        hint: Option<String>,
    ) -> Response {
        if password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto password must not be empty".to_owned(),
            };
        }
        // Manual translation — contract forbids From/Into between
        // pcloud-ipc and pcloud-crypto so the two surfaces can evolve
        // independently.
        let wire_backend = match backend {
            CryptoBackendIpc::PclsyncCompat => pcloud_crypto::CryptoBackend::PclsyncCompat,
            CryptoBackendIpc::Enhanced => pcloud_crypto::CryptoBackend::Enhanced,
        };
        if matches!(wire_backend, pcloud_crypto::CryptoBackend::Enhanced)
            && !acknowledge_not_interop
        {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message:
                    "enhanced backend requires explicit acknowledgement via acknowledge_not_interop"
                        .to_owned(),
            };
        }

        log::info!("crypto setup: backend={}", wire_backend);

        match wire_backend {
            pcloud_crypto::CryptoBackend::PclsyncCompat => {
                self.setup_crypto_v2_pclsync_compat(password, hint)
            }
            pcloud_crypto::CryptoBackend::Enhanced => {
                let result = self.crypto.setup_with_backend(
                    password,
                    hint,
                    pcloud_crypto::CryptoBackend::Enhanced,
                );
                self.metric_sync_crypto_state();
                match result {
                    Ok(()) => self.audited_response(
                        "crypto.setup_v2",
                        Some("backend=enhanced".to_owned()),
                        format!(
                            "crypto setup ok (backend={})",
                            pcloud_crypto::CryptoBackend::Enhanced
                        ),
                    ),
                    Err(err) => Response {
                        status: ResponseStatus::Conflict,
                        message: err.to_string(),
                    },
                }
            }
        }
    }

    /// PclsyncCompat half of [`Self::setup_crypto_v2`]: run local setup,
    /// base64-encode the sealed `priv_key_ver1` / `pub_key_ver1` blobs,
    /// then upload them via `crypto_setuserkeys`. On server error we
    /// surface the server-reported result code in the response message;
    /// the shell state is left as-is (local setup stays committed, same
    /// as the C behavior: the retry resends the same sealed blobs and
    /// the server-side overwrite semantics make it idempotent).
    fn setup_crypto_v2_pclsync_compat(
        &mut self,
        password: SecretString,
        hint: Option<String>,
    ) -> Response {
        use base64::Engine as _;
        use base64::engine::general_purpose::STANDARD as B64;

        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "crypto setup (pclsync-compat) requires an authenticated session"
                        .to_owned(),
                };
            }
        };

        if let Err(err) = self.crypto.setup_with_backend(
            password,
            hint.clone(),
            pcloud_crypto::CryptoBackend::PclsyncCompat,
        ) {
            return Response {
                status: ResponseStatus::Conflict,
                message: err.to_string(),
            };
        }
        self.metric_sync_crypto_state();

        let (priv_b64, pub_b64) = {
            let profile = match self.crypto.pclsync_compat.as_ref() {
                Some(p) => p,
                None => {
                    return Response {
                        status: ResponseStatus::InternalError,
                        message: "crypto setup succeeded but PclsyncCompat profile is absent"
                            .to_owned(),
                    };
                }
            };
            (
                B64.encode(&profile.priv_key_ver1_blob),
                B64.encode(&profile.pub_key_ver1_blob),
            )
        };

        match self.crypto_runtime.set_user_keys(
            auth_token.expose_secret(),
            &priv_b64,
            &pub_b64,
            hint.as_deref(),
        ) {
            Ok(()) => self.audited_response(
                "crypto.setup_v2",
                Some("backend=pclsync-compat".to_owned()),
                format!(
                    "crypto setup ok (backend={})",
                    pcloud_crypto::CryptoBackend::PclsyncCompat
                ),
            ),
            Err(err) => {
                log::error!("crypto_setuserkeys failed: {err}");
                Response {
                    status: ResponseStatus::InternalError,
                    message: format!("crypto_setuserkeys failed: {err}"),
                }
            }
        }
    }

    /// Dispatch [`Request::CryptoGetFolderKey`]: require an unlocked
    /// PclsyncCompat shell, fetch the RSA-OAEP-wrapped sym-key from the
    /// server, RSA-OAEP-unwrap it locally, and cache it against
    /// `folder_id`.
    fn crypto_get_folder_key(&mut self, folder_id: u64) -> Response {
        if !matches!(
            self.crypto.effective_backend(),
            pcloud_crypto::CryptoBackend::PclsyncCompat
        ) {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto_getfolderkey is only valid on a PclsyncCompat shell".to_owned(),
            };
        }
        if !self.crypto.is_started() {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "crypto must be unlocked to fetch a folder key".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "crypto_getfolderkey requires an authenticated session".to_owned(),
                };
            }
        };
        let wrapped = match self
            .crypto_runtime
            .get_folder_key(auth_token.expose_secret(), folder_id)
        {
            Ok(bytes) => bytes,
            Err(err) => {
                return Response {
                    status: ResponseStatus::InternalError,
                    message: format!("crypto_getfolderkey failed: {err}"),
                };
            }
        };
        match self.crypto.unwrap_and_cache_folder_key(folder_id, &wrapped) {
            Ok(()) => self.audited_response(
                "crypto.folder_key.cached",
                Some(format!("folder_id={folder_id}")),
                format!("folder key cached: folder_id={folder_id}"),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to unwrap folder key (folder_id={folder_id}): {err}"),
            },
        }
    }

    /// Dispatch [`Request::CryptoGetFileKey`]: same shape as
    /// [`Self::crypto_get_folder_key`], but targeting a `file_id`. The
    /// server-reported file-version `hash` is threaded into the cache so
    /// a follow-up can gate on stale entries.
    fn crypto_get_file_key(&mut self, file_id: u64) -> Response {
        if !matches!(
            self.crypto.effective_backend(),
            pcloud_crypto::CryptoBackend::PclsyncCompat
        ) {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto_getfilekey is only valid on a PclsyncCompat shell".to_owned(),
            };
        }
        if !self.crypto.is_started() {
            return Response {
                status: ResponseStatus::Unauthorized,
                message: "crypto must be unlocked to fetch a file key".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "crypto_getfilekey requires an authenticated session".to_owned(),
                };
            }
        };
        let (hash, wrapped) = match self
            .crypto_runtime
            .get_file_key(auth_token.expose_secret(), file_id)
        {
            Ok(pair) => pair,
            Err(err) => {
                return Response {
                    status: ResponseStatus::InternalError,
                    message: format!("crypto_getfilekey failed: {err}"),
                };
            }
        };
        match self
            .crypto
            .unwrap_and_cache_file_key(file_id, hash, &wrapped)
        {
            Ok(()) => self.audited_response(
                "crypto.file_key.cached",
                Some(format!("file_id={file_id}")),
                format!("file key cached: file_id={file_id}"),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to unwrap file key (file_id={file_id}): {err}"),
            },
        }
    }

    /// Auto-fetch retry wrapper around
    /// Auto-fetch retry wrapper around
    /// [`pcloud_crypto::CryptoShell::mkdir_with_context`] that recovers
    /// from a `FolderKeyNotCached` error by issuing `crypto_getfolderkey`
    /// for the parent folder, caching the unwrapped sym-key, and retrying
    /// the mkdir **once**. PclsyncCompat only; Enhanced shells bypass.
    ///
    /// This is the filename-side analog of
    /// [`Self::seal_sector_with_autofetch`]. Mirrors the C client's
    /// lazy-load behavior in `pcryptofolder.c:826` where a mkdir under
    /// an un-cached parent triggers a `download_fldr_enckey` round-trip
    /// before the local encode proceeds.
    pub fn mkdir_with_autofetch(
        &mut self,
        parent_folder_id: Option<u64>,
        name: &str,
        local_folder_id: Option<u64>,
    ) -> Result<pcloud_crypto::CreatedCryptoFolder, pcloud_crypto::CryptoError> {
        match self
            .crypto
            .mkdir_with_context(parent_folder_id, name, local_folder_id)
        {
            Ok(created) => Ok(created),
            Err(pcloud_crypto::CryptoError::FolderKeyNotCached { folder_id }) => {
                if !matches!(
                    self.crypto.effective_backend(),
                    pcloud_crypto::CryptoBackend::PclsyncCompat
                ) || !self.crypto.is_started()
                {
                    return Err(pcloud_crypto::CryptoError::FolderKeyNotCached { folder_id });
                }
                let auth_token = self
                    .auth
                    .snapshot()
                    .auth_token
                    .as_ref()
                    .map(SecretString::clone_secret)
                    .ok_or(pcloud_crypto::CryptoError::Locked)?;
                let wrapped = self
                    .crypto_runtime
                    .get_folder_key(auth_token.expose_secret(), folder_id)
                    .map_err(|_| pcloud_crypto::CryptoError::PclsyncCompat)?;
                self.crypto
                    .unwrap_and_cache_folder_key(folder_id, &wrapped)?;
                self.crypto
                    .mkdir_with_context(parent_folder_id, name, local_folder_id)
            }
            Err(other) => Err(other),
        }
    }

    /// Auto-fetch retry wrapper around
    /// [`pcloud_crypto::CryptoShell::seal_sector_with_context`].
    ///
    /// PclsyncCompat only. If the first seal attempt fails with
    /// [`pcloud_crypto::CryptoError::FileKeyNotCached`] and the shell is
    /// unlocked, issue `crypto_getfilekey`, cache the unwrapped key,
    /// and retry **once**. A second failure is surfaced verbatim to the
    /// caller — we do not loop. Enhanced shells bypass the auto-fetch
    /// entirely (their sector keys are derived locally from a caller
    /// seed, so a `FileKeyNotCached` error is unreachable there).
    ///
    /// TODO(bd-1du.10): extend with a chunked streaming path once the
    /// multi-GiB writeback lands under `bd-1du.4`.
    pub fn seal_sector_with_autofetch(
        &mut self,
        file_seed: &[u8],
        sector_index: u64,
        plaintext: &[u8],
        context: pcloud_crypto::SectorContext,
    ) -> Result<pcloud_crypto::SealedSectorFrame, pcloud_crypto::CryptoError> {
        // First attempt.
        match self
            .crypto
            .seal_sector_with_context(file_seed, sector_index, plaintext, context)
        {
            Ok(sealed) => Ok(sealed),
            Err(pcloud_crypto::CryptoError::FileKeyNotCached { file_id }) => {
                // Only attempt an auto-fetch on PclsyncCompat + unlocked
                // shell + authenticated session. Otherwise propagate
                // the original error so the caller sees a stable
                // taxonomy.
                if !matches!(
                    self.crypto.effective_backend(),
                    pcloud_crypto::CryptoBackend::PclsyncCompat
                ) || !self.crypto.is_started()
                {
                    return Err(pcloud_crypto::CryptoError::FileKeyNotCached { file_id });
                }
                let auth_token = self
                    .auth
                    .snapshot()
                    .auth_token
                    .as_ref()
                    .map(SecretString::clone_secret)
                    .ok_or(pcloud_crypto::CryptoError::Locked)?;
                let (hash, wrapped) = self
                    .crypto_runtime
                    .get_file_key(auth_token.expose_secret(), file_id)
                    .map_err(|_| pcloud_crypto::CryptoError::PclsyncCompat)?;
                self.crypto
                    .unwrap_and_cache_file_key(file_id, hash, &wrapped)?;
                // Retry once. Any failure here is the final answer.
                self.crypto
                    .seal_sector_with_context(file_seed, sector_index, plaintext, context)
            }
            Err(other) => Err(other),
        }
    }

    fn crypto_status(&self) -> Response {
        Response {
            status: ResponseStatus::Ok,
            message: format!(
                "crypto: backend={}, setup={}, started={}, state={:?}, folders={}, hint={}, policy_safe={}",
                self.crypto.effective_backend(),
                self.crypto.is_setup(),
                self.crypto.is_started(),
                self.crypto.unlock_state,
                self.crypto.folders.len(),
                self.crypto.get_hint().unwrap_or("<none>"),
                self.crypto.policy.is_safe()
            ),
        }
    }

    fn crypto_reset(&mut self) -> Response {
        self.crypto.reset();
        self.metric_sync_crypto_state();
        self.audited_response(
            "crypto.reset",
            None,
            format!(
                "crypto reset (state={:?}, folders={})",
                self.crypto.unlock_state,
                self.crypto.folders.len()
            ),
        )
    }

    /// `psync_crypto_priv_key_flags` equivalent — returns the flags value
    /// in the response message as `flags=<decimal>`.
    fn crypto_priv_key_flags(&self) -> Response {
        Response {
            status: ResponseStatus::Ok,
            message: format!("flags={}", self.crypto.priv_key_flags()),
        }
    }

    /// `psync_crypto_crypto_send_change_user_private` equivalent. Requires
    /// an authenticated session; never echoes the auth token back.
    fn send_crypto_change_user_private(&mut self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "crypto send-change-user-private requires an authenticated session"
                        .to_owned(),
                };
            }
        };

        match self
            .crypto_runtime
            .send_change_user_private(auth_token.expose_secret())
        {
            Ok(()) => self.audited_response(
                "crypto.send_change_user_private",
                None,
                "server accepted code-send request".to_owned(),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("crypto_sendchangeuserprivate failed: {err}"),
            },
        }
    }

    fn change_crypto_password(
        &mut self,
        old_password: SecretString,
        new_password: SecretString,
        hint: String,
        code: String,
        flags: u64,
    ) -> Response {
        if old_password.is_empty() || new_password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto passwords must not be empty".to_owned(),
            };
        }
        if code.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto password change requires a confirmation code".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "crypto change-password requires an authenticated session".to_owned(),
                };
            }
        };

        let rekeyed = match self
            .crypto
            .change_password(old_password, new_password, flags)
        {
            Ok(out) => out,
            Err(pcloud_crypto::CryptoError::WrongPassword) => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "wrong crypto password".to_owned(),
                };
            }
            Err(err) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: err.to_string(),
                };
            }
        };

        self.upload_reencoded_private_key(auth_token, rekeyed, hint, code, flags)
    }

    fn change_crypto_password_unlocked(
        &mut self,
        new_password: SecretString,
        hint: String,
        code: String,
        flags: u64,
    ) -> Response {
        if new_password.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto password must not be empty".to_owned(),
            };
        }
        if code.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "crypto password change requires a confirmation code".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "crypto change-password requires an authenticated session".to_owned(),
                };
            }
        };

        let rekeyed = match self.crypto.change_password_unlocked(new_password, flags) {
            Ok(out) => out,
            Err(pcloud_crypto::CryptoError::Locked) => {
                return Response {
                    status: ResponseStatus::Unauthorized,
                    message: "crypto must be unlocked to use the unlocked password-change flow"
                        .to_owned(),
                };
            }
            Err(err) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: err.to_string(),
                };
            }
        };

        self.upload_reencoded_private_key(auth_token, rekeyed, hint, code, flags)
    }

    fn upload_reencoded_private_key(
        &mut self,
        auth_token: SecretString,
        rekeyed: pcloud_crypto::ReencodedPrivateKey,
        hint: String,
        code: String,
        flags: u64,
    ) -> Response {
        match self.crypto_runtime.change_user_private(
            auth_token.expose_secret(),
            &rekeyed.private_key_hex,
            &rekeyed.signature_hex,
            &hint,
            &code,
        ) {
            Ok(()) => self.audited_response(
                "crypto.change_password",
                Some(format!("flags={flags}")),
                format!(
                    "crypto password rotated (flags={flags}, state={:?})",
                    self.crypto.unlock_state
                ),
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("crypto_changeuserprivate failed: {err}"),
            },
        }
    }

    fn set_auth_persistence(&mut self, enabled: bool) -> Response {
        let previous_enabled = self.config.features.durable_auth_tokens_enabled;
        let previous_preference = self
            .store
            .repositories
            .preferences
            .durable_auth_tokens_enabled;
        let previous_vault_token = match load_token(&self.config.paths.auth_token_vault_path()) {
            Ok(token) => token,
            Err(err) => {
                return Response {
                    status: ResponseStatus::InternalError,
                    message: format!("failed to inspect existing auth vault state: {err}"),
                };
            }
        };
        self.config.features.durable_auth_tokens_enabled = enabled;
        self.store
            .repositories
            .preferences
            .durable_auth_tokens_enabled = Some(enabled);

        if let Err(err) = persist_profile(&self.store) {
            self.config.features.durable_auth_tokens_enabled = previous_enabled;
            self.store
                .repositories
                .preferences
                .durable_auth_tokens_enabled = previous_preference;
            return Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to persist authsave preference: {err}"),
            };
        }

        if let Err(err) = self.sync_auth_vault() {
            self.config.features.durable_auth_tokens_enabled = previous_enabled;
            self.store
                .repositories
                .preferences
                .durable_auth_tokens_enabled = previous_preference;
            // Audit finding M1: surface rollback failures instead of silently
            // dropping them. If either rollback step fails the on-disk state
            // may diverge from the in-memory state we are returning to the
            // client, so record the compound failure and escalate in the
            // response so the client is not misled about durable state.
            let mut rollback_errors: Vec<String> = Vec::new();
            if let Err(rollback_err) = persist_profile(&self.store) {
                log::error!(
                    "authsave rollback: persist_profile failed after sync_auth_vault error: {rollback_err}"
                );
                rollback_errors.push(format!("persist_profile: {rollback_err}"));
            }
            if let Err(rollback_err) = self.restore_vault_state(previous_vault_token) {
                log::error!(
                    "authsave rollback: restore_vault_state failed after sync_auth_vault error: {rollback_err}"
                );
                rollback_errors.push(format!("restore_vault_state: {rollback_err}"));
            }
            let message = if rollback_errors.is_empty() {
                format!("failed to apply authsave preference: {err}")
            } else {
                format!(
                    "failed to apply authsave preference: {err}; rollback incomplete: {}",
                    rollback_errors.join("; ")
                )
            };
            return Response {
                status: ResponseStatus::InternalError,
                message,
            };
        }

        self.audited_response(
            "auth.authsave",
            Some(format!("enabled={enabled}")),
            format!("authsave {}", if enabled { "enabled" } else { "disabled" }),
        )
    }

    fn fetch_userinfo(&mut self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "userinfo requires an authenticated session".to_owned(),
                };
            }
        };

        match self.auth_runtime.userinfo(auth_token) {
            Ok(userinfo) => {
                if let Err(err) = self
                    .auth
                    .update_userinfo(userinfo.user_id.map(UserId::new), userinfo.email.clone())
                {
                    return Response {
                        status: ResponseStatus::Conflict,
                        message: err.to_string(),
                    };
                }

                if let Err(err) = self.sync_account_store() {
                    return Response {
                        status: ResponseStatus::InternalError,
                        message: format!("failed to persist account state: {err}"),
                    };
                }

                let payload = serde_json::json!({
                    "user_id": self.auth.snapshot().authenticated_user.map(|id| id.get()),
                    "email": self.auth.snapshot().email,
                    "quota": userinfo.quota,
                    "usedquota": userinfo.used_quota,
                    "premium": userinfo.premium,
                    "premiumexpires": userinfo.premium_expires,
                    "emailverified": userinfo.email_verified,
                    "plan": userinfo.plan,
                });
                self.audited_response("auth.userinfo", None, payload.to_string())
            }
            Err(err) => map_auth_flow_error(err),
        }
    }

    fn send_two_factor_sms(&mut self) -> Response {
        let token_present = match self.auth.snapshot().pending_challenge.as_ref() {
            Some(challenge) if !challenge.token.expose_secret().is_empty() => true,
            Some(_) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "two-factor sms delivery is unavailable when the backend does not return a challenge token".to_owned(),
                }
            }
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "two-factor sms delivery requires a pending challenge".to_owned(),
                }
            }
        };

        match self.auth_runtime.send_two_factor_sms(&self.auth) {
            Ok(delivery) => self.audited_response(
                "auth.tfa.sms",
                None,
                format!(
                    "tfa sms requested: country_code={:?}, phone_number={:?}, token_present={}",
                    delivery.country_code, delivery.phone_number, token_present
                ),
            ),
            Err(err) => map_auth_flow_error(err),
        }
    }

    fn send_two_factor_notification(&mut self) -> Response {
        let token_present = match self.auth.snapshot().pending_challenge.as_ref() {
            Some(challenge) if !challenge.token.expose_secret().is_empty() => true,
            Some(_) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "two-factor notification delivery is unavailable when the backend does not return a challenge token".to_owned(),
                }
            }
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "two-factor notification delivery requires a pending challenge".to_owned(),
                }
            }
        };

        match self.auth_runtime.send_two_factor_notification(&self.auth) {
            Ok(delivery) => {
                let devices = delivery
                    .devices
                    .iter()
                    .map(|device| {
                        device
                            .name
                            .clone()
                            .unwrap_or_else(|| "<unnamed>".to_owned())
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                self.audited_response(
                    "auth.tfa.notification",
                    None,
                    format!(
                        "tfa notification requested: devices=[{}], token_present={}",
                        devices, token_present
                    ),
                )
            }
            Err(err) => map_auth_flow_error(err),
        }
    }

    fn pending_transfers(&self) -> Response {
        let pending = self.engine.scheduler.queued_operations.len()
            + self.engine.uploads.active_count()
            + self.engine.downloads.active_count();
        Response {
            status: ResponseStatus::Ok,
            message: format!(
                "pending: total={}, queued={}, active_uploads={}, active_downloads={}, completed_uploads={}, completed_downloads={}, failed_uploads={}, failed_downloads={}",
                pending,
                self.engine.scheduler.queued_operations.len(),
                self.engine.uploads.active_count(),
                self.engine.downloads.active_count(),
                self.engine.uploads.completed_count(),
                self.engine.downloads.completed_count(),
                self.engine.uploads.failed_count(),
                self.engine.downloads.failed_count(),
            ),
        }
    }

    fn list_sync_roots(&self) -> Response {
        let roots = &self.store.repositories.sync_graph.tracked_sync_roots;
        let rendered = if roots.is_empty() {
            "[]".to_owned()
        } else {
            roots
                .iter()
                .map(|root| {
                    format!(
                        "{{id={}, local=\"{}\", remote=\"{}\", paused={}}}",
                        root.sync_id.get(),
                        root.local_path,
                        root.remote_path,
                        root.paused
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        Response {
            status: ResponseStatus::Ok,
            message: format!("sync roots: count={}, {}", roots.len(), rendered),
        }
    }

    fn list_public_links(&mut self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link listing requires an authenticated session".to_owned(),
                };
            }
        };

        match self.public_link_runtime.list_public_links(auth_token) {
            Ok(links) => {
                // Machine-readable JSON payload embedded in
                // `Response.message` so `--json` pipelines see a
                // structured `.links[]` array instead of the legacy
                // free-form "{id=…, code=…}" rendering. Every historic
                // field is preserved; the shape matches the
                // `list-links` manpage recipe.
                let payload = serde_json::json!({
                    "count": links.len(),
                    "links": links.iter().map(|link| serde_json::json!({
                        "id": link.link_id,
                        "code": link.code,
                        "name": link.name,
                        "is_folder": link.is_folder,
                        "is_upload": link.is_upload,
                        "link": link.link,
                    })).collect::<Vec<_>>(),
                });
                self.audited_response(
                    "publinks.list",
                    Some(format!("count={}", links.len())),
                    payload.to_string(),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn show_public_link(&mut self, code: String) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link inspection requires an authenticated session".to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .show_public_link(auth_token, code.clone())
        {
            Ok(contents) => {
                let rendered = if contents.entries.is_empty() {
                    "[]".to_owned()
                } else {
                    contents
                        .entries
                        .iter()
                        .map(|entry| {
                            format!(
                                "{{name=\"{}\", is_folder={}, item_id={}}}",
                                entry.name, entry.is_folder, entry.item_id
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.audited_response(
                    "publinks.show",
                    Some(format!("code={code} count={}", contents.entries.len())),
                    format!(
                        "public link contents: code=\"{}\", count={}, {}",
                        contents.code,
                        contents.entries.len(),
                        rendered
                    ),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn delete_public_link(&mut self, link_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link deletion requires an authenticated session".to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .delete_public_link(auth_token, link_id)
        {
            Ok(()) => self.audited_response(
                "publinks.delete",
                Some(format!("link_id={link_id}")),
                format!("public link deleted: id={link_id}"),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    /// Delete a public link identified by its short share code.
    ///
    /// Resolves the code to a numeric link id by scanning the current
    /// account's `list_public_links` response, then delegates to the
    /// existing id-form delete path. The extra round-trip is a
    /// deliberate trade: keeps the CLI one-command UX
    /// (`pcloudc delete-link <CODE>`) without requiring a server-side
    /// endpoint change or a stateful CLI cache.
    fn delete_public_link_by_code(&mut self, code: String) -> Response {
        let trimmed = code.trim().to_owned();
        if trimmed.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "delete-link: code must not be empty".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link deletion requires an authenticated session".to_owned(),
                };
            }
        };
        let links = match self
            .public_link_runtime
            .list_public_links(auth_token.clone_secret())
        {
            Ok(links) => links,
            Err(err) => return map_public_link_error(err),
        };
        let Some(hit) = links.iter().find(|l| l.code == trimmed) else {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: format!("public link code not found: {trimmed}"),
            };
        };
        let link_id = hit.link_id;
        match self
            .public_link_runtime
            .delete_public_link(auth_token, link_id)
        {
            Ok(()) => self.audited_response(
                "publinks.delete",
                Some(format!("link_id={link_id} code={trimmed}")),
                format!("public link deleted: id={link_id}"),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn create_file_public_link(&mut self, path: String) -> Response {
        self.create_public_link(path, false)
    }

    fn create_folder_public_link(&mut self, path: String) -> Response {
        self.create_public_link(path, true)
    }

    fn create_public_link(&mut self, path: String, is_folder: bool) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "public link path must not be empty".to_owned(),
            };
        }

        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link creation requires an authenticated session".to_owned(),
                };
            }
        };

        // Data-residency enforcement (docs/enterprise/data-residency.md §5).
        // A public link published from a disallowed region would leak
        // regulated data through the share URL; refuse in strict mode.
        let region = self.active_host_region();
        if let Some(refusal) =
            self.check_residency(pcloud_backends::residency::ACTION_UPLOAD_CREATE, region)
        {
            return refusal;
        }

        let created = if is_folder {
            self.public_link_runtime
                .create_folder_public_link(auth_token, path.clone())
        } else {
            self.public_link_runtime
                .create_file_public_link(auth_token, path.clone())
        };

        match created {
            Ok(created) => {
                // JSON payload lets the `--json` manpage recipes pipe
                // the response through `jq '.message | fromjson | .code'`
                // without regex-scraping. `code` is derived from the
                // trailing path segment of the share URL (the pCloud
                // short-code convention), not from the protocol
                // response directly — older SDK layers did not expose
                // it as a first-class field.
                let code = short_code_from_link(&created.link);
                let payload = serde_json::json!({
                    "id": created.link_id,
                    "code": code,
                    "is_folder": created.is_folder,
                    "link": created.link,
                });
                self.audited_response(
                    if is_folder {
                        "publinks.create_folder"
                    } else {
                        "publinks.create_file"
                    },
                    Some(format!("path={} link_id={}", path, created.link_id)),
                    payload.to_string(),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn change_public_link_expire(&mut self, link_id: u64, expire: Option<u64>) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link change requires an authenticated session".to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .change_public_link_expire(auth_token, link_id, expire)
        {
            Ok(()) => self.audited_response(
                "publinks.change_expire",
                Some(format!("link_id={} expire={:?}", link_id, expire)),
                match expire {
                    Some(expire) => {
                        format!(
                            "public link expire updated: id={}, expire={}",
                            link_id, expire
                        )
                    }
                    None => format!("public link expire cleared: id={}", link_id),
                },
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn change_public_link_password(
        &mut self,
        link_id: u64,
        password: Option<SecretString>,
    ) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link change requires an authenticated session".to_owned(),
                };
            }
        };

        // ncx.66: capture `action` + `has_password` before moving the
        // `SecretString` into the backend so the audit/response strings
        // never see the cleartext password or force a `.clone()` on
        // secret material.
        let has_password = password.is_some();
        match self
            .public_link_runtime
            .change_public_link_password(auth_token, link_id, password)
        {
            Ok(()) => self.audited_response(
                "publinks.change_password",
                Some(format!(
                    "link_id={} action={}",
                    link_id,
                    if has_password { "set" } else { "clear" }
                )),
                if has_password {
                    format!("public link password updated: id={}", link_id)
                } else {
                    format!("public link password cleared: id={}", link_id)
                },
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn change_public_link_upload(
        &mut self,
        link_id: u64,
        policy: PublicLinkUploadPolicy,
    ) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link change requires an authenticated session".to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .change_public_link_upload(auth_token, link_id, policy)
        {
            Ok(()) => self.audited_response(
                "publinks.change_upload",
                Some(format!("link_id={} policy={:?}", link_id, policy)),
                format!(
                    "public link upload policy updated: id={}, policy={:?}",
                    link_id, policy
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn list_upload_links(&mut self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "upload link listing requires an authenticated session".to_owned(),
                };
            }
        };

        match self.public_link_runtime.list_upload_links(auth_token) {
            Ok(links) => {
                let rendered = if links.is_empty() {
                    "[]".to_owned()
                } else {
                    links
                        .iter()
                        .map(|link| {
                            format!(
                                "{{id={}, code=\"{}\", name=\"{}\", comment=\"{}\", files={}, maxspace={:?}, link=\"{}\"}}",
                                link.upload_link_id,
                                link.code,
                                link.name,
                                link.comment,
                                link.files,
                                link.maxspace,
                                link.link
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.audited_response(
                    "uploadlinks.list",
                    Some(format!("count={}", links.len())),
                    format!("upload links: count={}, {}", links.len(), rendered),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn create_upload_link(
        &mut self,
        path: String,
        comment: String,
        expire: Option<u64>,
        maxspace: Option<u64>,
        maxfiles: Option<u64>,
    ) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "upload link path must not be empty".to_owned(),
            };
        }
        // Empty comments are accepted (matches C client and the
        // bare-CLI form `pcloudc create-upload-link <PATH>`); the
        // backend records the empty string verbatim.

        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "upload link creation requires an authenticated session".to_owned(),
                };
            }
        };

        // Data-residency enforcement: upload links are write-side
        // surfaces (third parties push data into the operator's tenant)
        // so a link published from a disallowed region is at least as
        // bad as a public read link. Refuse in strict mode.
        let region = self.active_host_region();
        if let Some(refusal) =
            self.check_residency(pcloud_backends::residency::ACTION_UPLOAD_CREATE, region)
        {
            return refusal;
        }

        match self.public_link_runtime.create_upload_link(
            auth_token,
            path.clone(),
            comment,
            expire,
            maxspace,
            maxfiles,
        ) {
            Ok(created) => {
                let code = short_code_from_link(&created.link);
                let payload = serde_json::json!({
                    "id": created.upload_link_id,
                    "code": code,
                    "is_folder": true,
                    "link": created.link,
                });
                self.audited_response(
                    "uploadlinks.create",
                    Some(format!(
                        "path={} upload_link_id={} expire={:?} maxspace={:?} maxfiles={:?}",
                        path, created.upload_link_id, expire, maxspace, maxfiles
                    )),
                    payload.to_string(),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn delete_upload_link(&mut self, upload_link_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "upload link deletion requires an authenticated session".to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .delete_upload_link(auth_token, upload_link_id)
        {
            Ok(()) => self.audited_response(
                "uploadlinks.delete",
                Some(format!("upload_link_id={upload_link_id}")),
                format!("upload link deleted: id={upload_link_id}"),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn create_tree_public_link(
        &mut self,
        name: String,
        root_folder_id: Option<u64>,
        folder_ids_csv: Option<String>,
        file_ids_csv: Option<String>,
        expire: Option<u64>,
        maxdownloads: Option<u64>,
        maxtraffic: Option<u64>,
    ) -> Response {
        if name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "tree link name must not be empty".to_owned(),
            };
        }
        if root_folder_id.is_none()
            && folder_ids_csv
                .as_ref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            && file_ids_csv
                .as_ref()
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
        {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "tree link requires at least one target id".to_owned(),
            };
        }

        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "tree link creation requires an authenticated session".to_owned(),
                };
            }
        };

        match self.public_link_runtime.create_tree_public_link(
            auth_token,
            name.clone(),
            root_folder_id,
            folder_ids_csv.clone(),
            file_ids_csv.clone(),
            expire,
            maxdownloads,
            maxtraffic,
        ) {
            Ok(created) => self.audited_response(
                "publinks.create_tree",
                Some(format!(
                    "name={} root_folder_id={:?} folder_ids_csv={:?} file_ids_csv={:?}",
                    name, root_folder_id, folder_ids_csv, file_ids_csv
                )),
                format!(
                    "tree public link created: id={}, name=\"{}\", link=\"{}\"",
                    created.link_id, created.name, created.link
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn list_public_link_access(&mut self, link_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link access listing requires an authenticated session"
                        .to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .list_public_link_access(auth_token, link_id)
        {
            Ok(entries) => {
                let rendered = if entries.is_empty() {
                    "[]".to_owned()
                } else {
                    entries
                        .iter()
                        .map(|entry| {
                            format!(
                                "{{email=\"{}\", receiver_id={}}}",
                                entry.email, entry.receiver_id
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.audited_response(
                    "publinks.list_access",
                    Some(format!("link_id={} count={}", link_id, entries.len())),
                    format!(
                        "public link access: link_id={}, count={}, {}",
                        link_id,
                        entries.len(),
                        rendered
                    ),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn add_public_link_access(&mut self, link_id: u64, email: String) -> Response {
        if email.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "public link access email must not be empty".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link access changes require an authenticated session"
                        .to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .add_public_link_access(auth_token, link_id, email.clone())
        {
            Ok(()) => self.audited_response(
                "publinks.add_access",
                Some(format!("link_id={} email={}", link_id, email)),
                format!(
                    "public link access granted: link_id={}, email=\"{}\"",
                    link_id, email
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn remove_public_link_access(&mut self, link_id: u64, receiver_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "public link access changes require an authenticated session"
                        .to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .remove_public_link_access(auth_token, link_id, receiver_id)
        {
            Ok(()) => self.audited_response(
                "publinks.remove_access",
                Some(format!("link_id={} receiver_id={}", link_id, receiver_id)),
                format!(
                    "public link access removed: link_id={}, receiver_id={}",
                    link_id, receiver_id
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn list_bookmarks(&mut self) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "bookmark listing requires an authenticated session".to_owned(),
                };
            }
        };

        match self.public_link_runtime.list_bookmarks(auth_token) {
            Ok(entries) => {
                let rendered = if entries.is_empty() {
                    "[]".to_owned()
                } else {
                    entries
                        .iter()
                        .map(|entry| {
                            format!(
                                "{{code=\"{}\", name=\"{}\", location_id={}}}",
                                entry.code, entry.name, entry.location_id
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.audited_response(
                    "publinks.list_bookmarks",
                    Some(format!("count={}", entries.len())),
                    format!("bookmarks: count={}, {}", entries.len(), rendered),
                )
            }
            Err(err) => map_public_link_error(err),
        }
    }

    fn remove_bookmark(&mut self, code: String, location_id: u64) -> Response {
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "bookmark changes require an authenticated session".to_owned(),
                };
            }
        };

        match self
            .public_link_runtime
            .remove_bookmark(auth_token, code.clone(), location_id)
        {
            Ok(()) => self.audited_response(
                "publinks.remove_bookmark",
                Some(format!("code={} location_id={}", code, location_id)),
                format!(
                    "bookmark removed: code=\"{}\", location_id={}",
                    code, location_id
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn change_bookmark(
        &mut self,
        code: String,
        location_id: u64,
        name: String,
        description: String,
    ) -> Response {
        if name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "bookmark name must not be empty".to_owned(),
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "bookmark changes require an authenticated session".to_owned(),
                };
            }
        };

        match self.public_link_runtime.change_bookmark(
            auth_token,
            code.clone(),
            location_id,
            name.clone(),
            description,
        ) {
            Ok(()) => self.audited_response(
                "publinks.change_bookmark",
                Some(format!(
                    "code={} location_id={} name={}",
                    code, location_id, name
                )),
                format!(
                    "bookmark changed: code=\"{}\", location_id={}, name=\"{}\"",
                    code, location_id, name
                ),
            ),
            Err(err) => map_public_link_error(err),
        }
    }

    fn add_sync_root(
        &mut self,
        local_path: String,
        remote_path: String,
        sync_type: Option<pcloud_model::sync::SyncType>,
    ) -> Response {
        if local_path.trim().is_empty() || remote_path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "sync root paths must not be empty".to_owned(),
            };
        }
        let canonical_local_path = match std::fs::canonicalize(&local_path) {
            Ok(path) if path.is_dir() => path,
            _ => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: format!(
                        "local sync root does not exist or is not a directory: {local_path}"
                    ),
                };
            }
        };
        let canonical_local_path_string = canonical_local_path.display().to_string();
        if let Some(existing) = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter()
            .find_map(|root| sync_root_path_conflict(&canonical_local_path, &root.local_path))
        {
            return Response {
                status: ResponseStatus::Conflict,
                message: existing,
            };
        }
        let auth_token = match self
            .auth
            .snapshot()
            .auth_token
            .as_ref()
            .map(SecretString::clone_secret)
        {
            Some(token) => token,
            None => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: "sync root addition requires an authenticated session".to_owned(),
                };
            }
        };
        let validated_remote = match self
            .sync_runtime
            .validate_remote_folder(auth_token, &remote_path)
        {
            Ok(folder) => folder,
            Err(err) => {
                return Response {
                    status: ResponseStatus::Conflict,
                    message: format!("remote sync root validation failed: {err}"),
                };
            }
        };
        // Data-residency enforcement (docs/enterprise/data-residency.md §5.1).
        // `ValidatedRemoteFolder` does not yet carry a per-folder API-server
        // hint, so we fall back to the active host region — which is
        // conservative: if the session is pinned to an EU host but the
        // allow-list is US-only, the check still fires. A future refactor
        // can switch to `FolderMetadataHint::api_server` once the folder
        // metadata client exposes it.
        let region = self.active_host_region();
        if let Some(refusal) =
            self.check_residency(pcloud_backends::residency::ACTION_SYNC_ROOT_ADD, region)
        {
            return refusal;
        }
        let next_id = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter()
            .map(|root| root.sync_id.get())
            .max()
            .unwrap_or(0)
            + 1;
        let resolved_sync_type = sync_type.unwrap_or(pcloud_model::sync::SyncType::Full);
        self.store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .push(SyncRootRecord {
                sync_id: pcloud_model::ids::SyncId::new(next_id),
                local_path: canonical_local_path_string.clone(),
                remote_path: validated_remote.path.clone(),
                paused: false,
                sync_type: resolved_sync_type,
            });
        match persist_profile(&self.store) {
            Ok(()) => {
                self.metric_sync_root_count();
                // Structured JSON in `message` — ADR-0017. Lets the CLI
                // parse `sync_id` / `sync_type` deterministically and
                // enables `--field sync_id --field sync_type` selectors
                // without a separate envelope carve-out.
                let sync_type_label = match resolved_sync_type {
                    pcloud_model::sync::SyncType::Full => "Full",
                    pcloud_model::sync::SyncType::UploadOnly => "UploadOnly",
                    pcloud_model::sync::SyncType::DownloadOnly => "DownloadOnly",
                    pcloud_model::sync::SyncType::BackupArchive => "BackupArchive",
                };
                let payload = serde_json::json!({
                    "sync_id": next_id,
                    "local_path": canonical_local_path_string,
                    "remote_path": validated_remote.path,
                    "remote_folder_id": validated_remote.folder_id.get(),
                    "sync_type": sync_type_label,
                });
                let message = serde_json::to_string(&payload)
                    .unwrap_or_else(|_| format!("sync root added: id={next_id}"));
                // Wake the background sync loop so it picks up the new
                // root without waiting for the next poll interval.
                if let Some(shared) = &self.sync_loop_shared {
                    shared.wake();
                }
                self.audited_response(
                    "sync.root.add",
                    Some(format!(
                        "id={next_id} local={} remote={} remote_folder_id={} sync_type={}",
                        canonical_local_path_string,
                        validated_remote.path,
                        validated_remote.folder_id.get(),
                        resolved_sync_type.label(),
                    )),
                    message,
                )
            }
            Err(err) => {
                self.store
                    .repositories
                    .sync_graph
                    .tracked_sync_roots
                    .retain(|root| root.sync_id.get() != next_id);
                Response {
                    status: ResponseStatus::InternalError,
                    message: format!("failed to persist sync root: {err}"),
                }
            }
        }
    }

    fn remove_sync_root(&mut self, sync_id: u64) -> Response {
        let before = self.store.repositories.sync_graph.tracked_sync_roots.len();
        let removed_local_path = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter()
            .find(|root| root.sync_id.get() == sync_id)
            .map(|root| root.local_path.clone());
        self.store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .retain(|root| root.sync_id.get() != sync_id);
        if self.store.repositories.sync_graph.tracked_sync_roots.len() == before {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!("sync root {} does not exist", sync_id),
            };
        }
        match persist_profile(&self.store) {
            Ok(()) => {
                self.metric_sync_root_count();
                let sid = pcloud_model::ids::SyncId::new(sync_id);
                self.engine.evict_sync_root(sid);
                if let Some(local_path) = removed_local_path.as_deref() {
                    // Drop staged bytes associated with this sync root's
                    // local prefix so a freshly added replacement root
                    // cannot pick up stale cached content from the removed
                    // tree. Mirrors the strong remove semantics in C
                    // `psync_delete_sync` which purges local sync state.
                    let prefix = local_path.to_owned();
                    self.cache
                        .staging
                        .files
                        .retain(|path, _| !path.starts_with(&prefix));
                    self.cache
                        .staging
                        .open_order
                        .retain(|path| !path.starts_with(&prefix));
                }
                // Wake the background sync loop so it re-evaluates
                // its root list immediately.
                if let Some(shared) = &self.sync_loop_shared {
                    shared.wake();
                }
                self.audited_response(
                    "sync.root.remove",
                    Some(format!(
                        "id={sync_id} local={}",
                        removed_local_path.as_deref().unwrap_or("<unknown>")
                    )),
                    format!("sync root removed: id={sync_id}"),
                )
            }
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to persist sync root removal: {err}"),
            },
        }
    }

    fn pause_sync_root(&mut self, sync_id: u64) -> Response {
        let Some(root) = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter_mut()
            .find(|root| root.sync_id.get() == sync_id)
        else {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!("sync root {sync_id} does not exist"),
            };
        };
        if root.paused {
            return Response {
                status: ResponseStatus::Ok,
                message: format!("sync root {sync_id} is already paused"),
            };
        }
        root.paused = true;
        if let Err(err) = persist_profile(&self.store) {
            // Best-effort rollback of the in-memory flag so we do not drift
            // from the persisted state on an active path.
            if let Some(root) = self
                .store
                .repositories
                .sync_graph
                .tracked_sync_roots
                .iter_mut()
                .find(|root| root.sync_id.get() == sync_id)
            {
                root.paused = false;
            }
            return Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to persist sync root pause: {err}"),
            };
        }
        self.engine
            .pause_sync_root(pcloud_model::ids::SyncId::new(sync_id));
        self.audited_response(
            "sync.root.pause",
            Some(format!("id={sync_id}")),
            format!("sync root paused: id={sync_id}"),
        )
    }

    fn resume_sync_root(&mut self, sync_id: u64) -> Response {
        let Some(root) = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter_mut()
            .find(|root| root.sync_id.get() == sync_id)
        else {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!("sync root {sync_id} does not exist"),
            };
        };
        if !root.paused {
            return Response {
                status: ResponseStatus::Ok,
                message: format!("sync root {sync_id} is not paused"),
            };
        }
        root.paused = false;
        if let Err(err) = persist_profile(&self.store) {
            if let Some(root) = self
                .store
                .repositories
                .sync_graph
                .tracked_sync_roots
                .iter_mut()
                .find(|root| root.sync_id.get() == sync_id)
            {
                root.paused = true;
            }
            return Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to persist sync root resume: {err}"),
            };
        }
        self.engine
            .resume_sync_root(pcloud_model::ids::SyncId::new(sync_id));
        // Wake the background sync loop so the resumed root is
        // processed on the next iteration without waiting.
        if let Some(shared) = &self.sync_loop_shared {
            shared.wake();
        }
        self.audited_response(
            "sync.root.resume",
            Some(format!("id={sync_id}")),
            format!("sync root resumed: id={sync_id}"),
        )
    }

    fn change_sync_root_type(
        &mut self,
        sync_id: u64,
        sync_type: pcloud_model::sync::SyncType,
    ) -> Response {
        let Some(root) = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter_mut()
            .find(|root| root.sync_id.get() == sync_id)
        else {
            return Response {
                status: ResponseStatus::Conflict,
                message: format!("sync root {sync_id} does not exist"),
            };
        };
        let previous = root.sync_type;
        if previous == sync_type {
            return Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "sync root {sync_id} already has sync type {}",
                    sync_type.label()
                ),
            };
        }
        root.sync_type = sync_type;
        if let Err(err) = persist_profile(&self.store) {
            if let Some(root) = self
                .store
                .repositories
                .sync_graph
                .tracked_sync_roots
                .iter_mut()
                .find(|root| root.sync_id.get() == sync_id)
            {
                root.sync_type = previous;
            }
            return Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to persist sync type change: {err}"),
            };
        }
        // Direction changes invalidate already-queued work because upload
        // and download plans may no longer be valid; the next scan/diff
        // cycle will rebuild the queue.
        self.engine
            .scheduler
            .evict_sync_id(pcloud_model::ids::SyncId::new(sync_id));
        self.audited_response(
            "sync.root.change_type",
            Some(format!(
                "id={sync_id} from={} to={}",
                previous.label(),
                sync_type.label()
            )),
            format!(
                "sync root {sync_id} sync type changed: {} -> {}",
                previous.label(),
                sync_type.label()
            ),
        )
    }

    fn suggest_sync_folders_at(&mut self, path: String, max: usize) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "suggestion path must not be empty".to_owned(),
            };
        }
        let base = Path::new(&path);
        match crate::sync_backend::suggest_sync_folders(base, max) {
            Ok(suggestions) => {
                let rendered = if suggestions.is_empty() {
                    "[]".to_owned()
                } else {
                    suggestions
                        .iter()
                        .map(|entry| {
                            format!(
                                "{{name=\"{}\", local=\"{}\", description=\"{}\", files={}}}",
                                entry.name, entry.local_path, entry.description, entry.file_count
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.audited_response(
                    "sync.root.suggestions",
                    Some(format!("count={}", suggestions.len())),
                    format!(
                        "sync suggestions: count={}, {}",
                        suggestions.len(),
                        rendered
                    ),
                )
            }
            Err(err) => Response {
                status: ResponseStatus::Conflict,
                message: format!("failed to scan {path}: {err}"),
            },
        }
    }

    fn check_folder_syncable(&mut self, path: String) -> Response {
        if path.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "path must not be empty".to_owned(),
            };
        }
        let existing_paths: Vec<String> = self
            .store
            .repositories
            .sync_graph
            .tracked_sync_roots
            .iter()
            .map(|root| root.local_path.clone())
            .collect();
        let existing_refs: Vec<&str> = existing_paths.iter().map(String::as_str).collect();
        match crate::sync_backend::classify_folder_syncability(
            Path::new(&path),
            &existing_refs,
            &crate::mount_discovery::MountDiscovery::default(),
            &crate::sync_backend::FolderSyncabilityOverrides::default(),
        ) {
            Ok(canonical) => Response {
                status: ResponseStatus::Ok,
                message: format!("folder is syncable: canonical=\"{}\"", canonical.display()),
            },
            Err(issue) => Response {
                status: ResponseStatus::Conflict,
                message: issue.message(),
            },
        }
    }

    /// Sub-task 2: render the current session lifecycle (expiry,
    /// last-used, refresh-in-flight) as a JSON-encoded
    /// [`pcloud_ipc::SessionStatusPayload`] in `Response.message`.
    /// Contains no secret material; safe to log at operator level.
    fn session_status(&self) -> Response {
        let lc = self.auth.lifecycle();
        let payload = pcloud_ipc::SessionStatusPayload {
            expires_at: lc.map(|lc| lc.expires_at),
            last_used_at: lc.map(|lc| lc.last_used_at),
            refresh_in_flight: self.session_supervisor.refresh_in_flight(),
        };
        match serde_json::to_string(&payload) {
            Ok(message) => Response {
                status: ResponseStatus::Ok,
                message,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to encode session status: {err}"),
            },
        }
    }

    fn auth_response(&mut self, event: pcloud_auth::AuthEvent) -> Response {
        self.metric_record_auth_event(&event);
        // SLO wiring (I15 hot-path call site #3).
        //
        // `auth.login.success_rate` is driven here because every auth
        // state transition passes through `auth_response`. Only terminal
        // login outcomes (success/failure) count — challenge-issued and
        // refresh events are not login attempts and must not poison the
        // SLI. Token-refresh outcomes are intentionally excluded so the
        // daily pass-rate reflects interactive login UX, not background
        // refresh noise.
        match &event {
            pcloud_auth::AuthEvent::LoginSucceeded { .. } => {
                self.observability.slo.observe_auth_login(true);
            }
            pcloud_auth::AuthEvent::LoginFailed { .. } => {
                self.observability.slo.observe_auth_login(false);
            }
            _ => {}
        }
        match event {
            pcloud_auth::AuthEvent::LoginSucceeded { .. } => {
                self.pending_password_auth = None;
                // Sub-task 3: attach session lifecycle so the supervisor
                // can classify refresh/idle/expiry. `credentials_retained`
                // is always `false`: this fork never persists the
                // password (see CLAUDE.md "Secrets"). Refresh therefore
                // relies on `userinfo?getauth=1` against the live token,
                // which is pCloud's native refresh surface.
                let now = self.session_supervisor.now_secs();
                let policy = self.session_supervisor.policy().clone();
                self.auth.attach_lifecycle(now, &policy, false);
            }
            pcloud_auth::AuthEvent::LoginFailed { .. } | pcloud_auth::AuthEvent::LoggedOut => {
                self.pending_password_auth = None;
            }
            pcloud_auth::AuthEvent::TwoFactorChallengeIssued
            | pcloud_auth::AuthEvent::LoginStarted => {}
            // Sub-task 3: refresh lifecycle events. `TokenRefreshed`
            // carries no secret material (see pcloud-auth/events.rs);
            // audit persistence is handled below via `audited_response`.
            pcloud_auth::AuthEvent::TokenRefreshed { .. }
            | pcloud_auth::AuthEvent::TokenRefreshExpired { .. }
            | pcloud_auth::AuthEvent::TokenRefreshTemporaryFailure { .. } => {}
        }
        if let Err(err) = self.persist_auth_state() {
            return Response {
                status: ResponseStatus::InternalError,
                message: format!("failed to persist auth state: {err}"),
            };
        }
        self.audited_response(
            "auth.event",
            Some(format!("{event:?}")),
            match event {
                pcloud_auth::AuthEvent::LoginFailed { message } => match message {
                    Some(message) => format!("auth failed: {message}"),
                    None => "auth failed".to_owned(),
                },
                other => format!("auth event: {:?}", other),
            },
        )
    }

    // ==== Observability hooks (feature = "metrics") ==========================
    //
    // These helpers are all cheap; when the feature is off they are no-ops.
    // They are deliberately separate from `audited_response` so the metric
    // wiring is auditable by reviewers and does not smuggle behaviour into
    // the audit path.

    #[cfg(feature = "metrics")]
    pub(crate) fn metric_record_auth_event(&mut self, event: &pcloud_auth::AuthEvent) {
        if let Some(result) = auth_result_from_event(event) {
            self.observability.families.record_auth(result);
        }
    }
    #[cfg(not(feature = "metrics"))]
    pub(crate) fn metric_record_auth_event(&mut self, _event: &pcloud_auth::AuthEvent) {}

    /// Refresh the `pcloud_sync_root_count` metric from the current
    /// store snapshot. Cheap; a no-op when the `metrics` feature is
    /// compiled out.
    #[cfg(feature = "metrics")]
    pub fn metric_sync_root_count(&mut self) {
        let n = self.store.repositories.sync_graph.tracked_sync_roots.len() as u64;
        self.observability.families.set_sync_root_count(n);
    }
    /// Refresh the `pcloud_sync_root_count` metric. No-op build when
    /// the `metrics` feature is disabled.
    #[cfg(not(feature = "metrics"))]
    pub fn metric_sync_root_count(&mut self) {}

    #[cfg(feature = "metrics")]
    pub(crate) fn metric_sync_crypto_state(&mut self) {
        let label = crypto_state_label(self.crypto.unlock_state);
        self.observability.families.set_crypto_lock_state(label);
    }
    #[cfg(not(feature = "metrics"))]
    pub(crate) fn metric_sync_crypto_state(&mut self) {}

    #[cfg(feature = "metrics")]
    pub(crate) fn metric_add_transfer_bytes(&mut self, direction: TransferDirection, bytes: u64) {
        self.observability
            .families
            .add_transfer_bytes(direction, bytes);
    }

    /// Notify the observability layer that an IPC client has connected.
    /// Idempotent and zero-cost when `metrics` is disabled. Safe to call
    /// from the serve loop before any owner-uid check is invalidated.
    pub fn on_ipc_client_connected(&mut self) {
        #[cfg(feature = "metrics")]
        {
            let n = self
                .observability
                .families
                .ipc_connected_clients
                .saturating_add(1);
            self.observability.families.set_connected_clients(n);
        }
    }

    /// Notify the observability layer that an IPC client has disconnected.
    /// Clamps to zero to stay honest under spurious double-disconnect
    /// notifications (belt-and-braces, the serve layer should not emit
    /// them but an auditor should never see a negative client count).
    pub fn on_ipc_client_disconnected(&mut self) {
        #[cfg(feature = "metrics")]
        {
            let current = self.observability.families.ipc_connected_clients;
            let next = if current > 0 { current - 1 } else { 0 };
            self.observability.families.set_connected_clients(next);
        }
    }

    /// Fold the global panic counter into the metric gauge. Panic hooks
    /// run on arbitrary threads and cannot borrow `&mut self`, so the hook
    /// stores into a static atomic and this helper mirrors it onto the
    /// per-runtime metric families. Called from [`handle_request`].
    #[cfg(feature = "metrics")]
    pub(crate) fn metric_refresh_panic_count(&mut self) {
        use std::sync::atomic::Ordering;
        let global = PANIC_COUNT.load(Ordering::SeqCst);
        // Overwrite if the hook fired more times than we have observed.
        if global > self.observability.families.panic_count {
            let missed = global - self.observability.families.panic_count;
            for _ in 0..missed {
                self.observability.families.incr_panic();
            }
        }
    }

    fn record_audit_event(
        &mut self,
        category: impl Into<String>,
        details: Option<String>,
    ) -> Result<(), pcloud_store::StoreError> {
        let event = self.observability.record_event(category, details);
        append_audit_event(&mut self.store, &event.category, event.details.as_deref())
    }

    /// Tier-2 HA status probe — returns the current
    /// [`crate::ha_lease::HaStatusPayload`] as JSON in
    /// `Response::message`. Always succeeds; a disabled HA posture
    /// returns `mode = "disabled"` and null metadata. Passive daemons
    /// re-read the lease file each call so the reported
    /// `lease_age_s` is always fresh.
    pub fn ha_status(&mut self) -> Response {
        let payload = self.ha.status_payload();
        match serde_json::to_string(&payload) {
            Ok(json) => Response {
                status: ResponseStatus::Ok,
                message: json,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("ha-status: serialize failed: {err}"),
            },
        }
    }

    /// H14 PR4 — return current integrity sweeper progress as a JSON
    /// [`pcloud_ipc::IntegrityStatusPayload`] in `Response::message`.
    /// Always succeeds: a disabled sweeper reports zero progress and
    /// `enabled=false`.
    pub fn integrity_status(&mut self) -> Response {
        let snapshot = self.integrity_sweeper.progress_snapshot();
        let payload = pcloud_ipc::IntegrityStatusPayload {
            enabled: self.integrity_sweeper.is_enabled(),
            files_hashed: snapshot.files_hashed,
            bytes_hashed: snapshot.bytes_hashed,
            mismatches_found: snapshot.mismatches_found,
            throttled: snapshot.throttled,
            audit_drops: self.integrity_sweeper.audit_drop_count(),
        };
        match serde_json::to_string(&payload) {
            Ok(json) => Response {
                status: ResponseStatus::Ok,
                message: json,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("integrity-status: serialize failed: {err}"),
            },
        }
    }

    /// Return the scheduled audit-chain verifier status as a JSON
    /// [`pcloud_ipc::AuditVerifierStatusPayload`] in `Response::message`.
    /// Always succeeds: a disabled verifier reports `enabled=false`.
    pub fn audit_verifier_status(&self) -> Response {
        let payload = self.audit_verifier.status_snapshot();
        match serde_json::to_string(&payload) {
            Ok(json) => Response {
                status: ResponseStatus::Ok,
                message: json,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("audit-verifier-status: serialize failed: {err}"),
            },
        }
    }

    /// Return the background sync loop status as JSON in
    /// `Response::message`. Always safe to call.
    pub fn sync_loop_status(&self) -> Response {
        let status = match &self.sync_loop_shared {
            Some(shared) => shared.current_status(),
            None => crate::sync_loop::SyncLoopStatus {
                state: crate::sync_loop::SyncLoopState::Disabled,
                ..crate::sync_loop::SyncLoopStatus::default()
            },
        };
        match serde_json::to_string(&status) {
            Ok(json) => Response {
                status: ResponseStatus::Ok,
                message: json,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("sync-loop-status: serialize failed: {err}"),
            },
        }
    }

    /// List unresolved sync conflicts from the engine scheduler.
    /// Returns a JSON array of [`pcloud_ipc::ConflictEntry`] in
    /// `Response::message`. Always safe to call; returns an empty array
    /// when no conflicts are queued.
    pub fn list_conflicts(&self) -> Response {
        let entries: Vec<pcloud_ipc::ConflictEntry> = self
            .engine
            .list_unresolved_conflicts()
            .into_iter()
            .map(|(path, kind, sync_id)| pcloud_ipc::ConflictEntry {
                path,
                kind,
                sync_id,
            })
            .collect();
        match serde_json::to_string(&entries) {
            Ok(json) => Response {
                status: ResponseStatus::Ok,
                message: json,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("conflict-list: serialize failed: {err}"),
            },
        }
    }

    /// Resolve a specific conflict by path using the given policy string.
    /// Removes the conflict from the scheduler queue on success and emits
    /// an audit event.
    pub fn resolve_conflict(&mut self, path: String, policy: String) -> Response {
        match self.engine.resolve_conflict_by_path(&path, &policy) {
            Ok(resolution) => {
                let detail = format!("path={path}, policy={policy}");
                self.audited_response(
                    "sync.conflict.resolved",
                    Some(detail),
                    format!("conflict resolved: {resolution:?}"),
                )
            }
            Err(reason) => Response {
                status: ResponseStatus::InvalidRequest,
                message: reason,
            },
        }
    }

    /// Return the canonical SLO report as JSON in `Response::message`.
    ///
    /// Renders from the shared [`pcloud_observability::slo::Slo`]
    /// registry held on the observability shell. SLOs without enough
    /// samples report `status: "no_data"` so callers never conflate
    /// "quiet" with "healthy". Always succeeds; the response is a
    /// canonical [`pcloud_ipc::SloReportPayload`].
    pub fn get_slo(&self) -> Response {
        let snapshot = self.observability.slo.snapshot();
        let payload = pcloud_ipc::SloReportPayload {
            slos: snapshot
                .slos
                .iter()
                .map(|e| pcloud_ipc::SloReportEntry {
                    slo_name: e.slo_name.clone(),
                    target: e.target.clone(),
                    actual: e.actual.clone(),
                    status: match e.status {
                        pcloud_observability::slo::SloStatus::Ok => "ok".to_owned(),
                        pcloud_observability::slo::SloStatus::Violation => "violation".to_owned(),
                        pcloud_observability::slo::SloStatus::NoData => "no_data".to_owned(),
                    },
                })
                .collect(),
            pass: snapshot.pass,
        };
        match serde_json::to_string(&payload) {
            Ok(json) => Response {
                status: ResponseStatus::Ok,
                message: json,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("slo: serialize failed: {err}"),
            },
        }
    }

    /// H14 PR4 — synchronously trigger one sweep cycle and return the
    /// post-cycle progress snapshot. The PR4 walker is a placeholder
    /// (PR2/PR3 fill it in); the IPC envelope is wired here so the CLI
    /// surface stays stable.
    pub fn integrity_run_once(&mut self) -> Response {
        if !self.integrity_sweeper.is_enabled() {
            return Response {
                status: ResponseStatus::Unavailable,
                message: "integrity sweeper is not enabled — see [features.integrity_sweeper] enabled = true (bd-1du.4.6.1)".to_owned(),
            };
        }
        let snapshot = self.integrity_sweeper.run_once();
        let payload = pcloud_ipc::IntegrityStatusPayload {
            enabled: true,
            files_hashed: snapshot.files_hashed,
            bytes_hashed: snapshot.bytes_hashed,
            mismatches_found: snapshot.mismatches_found,
            throttled: snapshot.throttled,
            audit_drops: self.integrity_sweeper.audit_drop_count(),
        };
        match serde_json::to_string(&payload) {
            Ok(json) => self.audited_response(
                "integrity.run_once",
                Some(format!(
                    "files_hashed={} mismatches={} throttled={}",
                    snapshot.files_hashed, snapshot.mismatches_found, snapshot.throttled
                )),
                json,
            ),
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("integrity-run-once: serialize failed: {err}"),
            },
        }
    }

    /// H14 PR4 — append a glob pattern to the configured skip-list file
    /// and reload the in-memory glob set. Refuses when no
    /// `skip_list_path` is configured.
    pub fn integrity_skip(&mut self, path: String) -> Response {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "integrity skip requires a non-empty glob pattern".to_owned(),
            };
        }
        match self.integrity_sweeper.append_skip_path(trimmed) {
            Ok(()) => self.audited_response(
                "integrity.skip.add",
                // Hash the glob too — operators sometimes use absolute
                // paths as patterns and those are PII-equivalent.
                Some(format!(
                    "pattern_hash={}",
                    integrity_sweeper_service::path_hash_hex(Path::new(trimmed))
                )),
                format!("integrity skip-list updated: pattern={trimmed}"),
            ),
            Err(err) if err.kind() == std::io::ErrorKind::InvalidInput => Response {
                status: ResponseStatus::Unavailable,
                message: format!("integrity skip refused: {err}"),
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("integrity skip failed: {err}"),
            },
        }
    }

    /// Register a new operator-visible upload session (`Request::UploadCreate`).
    ///
    /// When `conflict_mode == Rename`, the caller has already agreed
    /// the daemon may pick a unique sibling name; we resolve it against
    /// the current in-memory registry (sessions under the same
    /// `parent_folder_id` that share the requested `remote_name`) so
    /// two simultaneous uploads of the same leaf name land in
    /// different slots. Collisions with server-side files are still the
    /// driver's responsibility to surface on `upload_save`.
    pub fn upload_session_create(
        &mut self,
        local_path: std::path::PathBuf,
        remote_name: String,
        parent_folder_id: Option<u64>,
        total_bytes: u64,
        conflict_mode: Option<pcloud_ipc::UploadConflictMode>,
    ) -> Response {
        use pcloud_backends::upload_sessions::{ConflictMode, pick_unique_name};

        if remote_name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "upload create requires a non-empty remote_name".to_owned(),
            };
        }
        let mode = match conflict_mode.unwrap_or_default() {
            pcloud_ipc::UploadConflictMode::Error => ConflictMode::Error,
            pcloud_ipc::UploadConflictMode::Overwrite => ConflictMode::Overwrite,
            pcloud_ipc::UploadConflictMode::Skip => ConflictMode::Skip,
            pcloud_ipc::UploadConflictMode::Rename => ConflictMode::Rename,
            _ => ConflictMode::Error,
        };

        let effective_name = if mode == ConflictMode::Rename {
            let existing: Vec<String> = self
                .upload_sessions
                .list()
                .iter()
                .filter(|s| s.parent_folder_id == parent_folder_id)
                .map(|s| s.remote_name.clone())
                .collect();
            pick_unique_name(&remote_name, existing)
        } else {
            remote_name.clone()
        };

        let session = self.upload_sessions.create(
            local_path,
            effective_name.clone(),
            parent_folder_id,
            total_bytes,
            mode,
        );
        let id = session.id;
        let message = serde_json::json!({
            "session_id": id,
            "remote_name": effective_name,
            "conflict_mode": mode_label(mode),
        })
        .to_string();
        self.audited_response(
            "upload.session.create",
            Some(format!("session_id={id} conflict={}", mode_label(mode))),
            message,
        )
    }

    /// Operator pause (`Request::UploadPause`). Idempotent against
    /// already-paused sessions.
    pub fn upload_session_pause(&mut self, session_id: u64) -> Response {
        upload_session_transition_response(self, "upload.session.pause", session_id, |reg| {
            reg.pause(session_id)
        })
    }

    /// Operator resume (`Request::UploadResume`). Rejects non-paused
    /// sessions with `Conflict`.
    pub fn upload_session_resume(&mut self, session_id: u64) -> Response {
        upload_session_transition_response(self, "upload.session.resume", session_id, |reg| {
            reg.resume(session_id)
        })
    }

    /// Operator cancel (`Request::UploadCancel`). Idempotent against
    /// already-cancelled sessions; rejects terminal `Completed|Failed`.
    pub fn upload_session_cancel(&mut self, session_id: u64) -> Response {
        upload_session_transition_response(self, "upload.session.cancel", session_id, |reg| {
            reg.cancel(session_id)
        })
    }

    /// Operator list (`Request::UploadList`). Returns every registered
    /// session as a JSON array in `Response::message`.
    pub fn upload_session_list(&mut self) -> Response {
        let sessions = self.upload_sessions.list();
        let payload = serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_owned());
        self.audited_response(
            "upload.session.list",
            Some(format!("count={}", sessions.len())),
            payload,
        )
    }

    /// Dispatch `Request::BackupSnapshot` onto the
    /// `pcloud_backends::snapshot` pipeline (zstd + SHA3 sidecar by
    /// default; optional GPG envelope). Each action emits a short JSON
    /// payload in `Response::message` that the CLI renders.
    pub fn handle_backup_snapshot(
        &mut self,
        action: pcloud_ipc::SnapshotAction,
        path: std::path::PathBuf,
        gpg_recipient: Option<String>,
        yes: bool,
        retention_days: Option<u32>,
        zstd_level: Option<i32>,
    ) -> Response {
        use pcloud_backends::snapshot as snap;
        use pcloud_ipc::SnapshotAction;

        match action {
            SnapshotAction::Create => {
                let level = zstd_level.unwrap_or(snap::ZSTD_DEFAULT_LEVEL);
                let mut opts = match snap::SnapshotOptions::with_zstd_level(level) {
                    Ok(o) => o,
                    Err(snap::SnapshotError::InvalidZstdLevel { got }) => {
                        return Response {
                            status: ResponseStatus::InvalidRequest,
                            message: format!(
                                "snapshot create: --zstd-level {got} is out of range (1..=22)"
                            ),
                        };
                    }
                    Err(err) => {
                        return Response {
                            status: ResponseStatus::InternalError,
                            message: format!("snapshot create: {err}"),
                        };
                    }
                };
                if let Some(r) = gpg_recipient.as_deref() {
                    opts = opts.with_gpg_recipient(r);
                }
                let store_path = self.config.paths.state_dir.join("store.sqlite3");
                let vault_path = self.config.paths.auth_token_vault_path();
                // The daemon's audit log is SQL-backed; the snapshot
                // format reserves an `audit.ndjson` slot for an on-disk
                // NDJSON audit trail that this daemon does not emit.
                // We stage an empty placeholder next to the store so
                // the snapshot manifest digest is well-defined; the
                // inner `store.sqlite3` payload carries the
                // authoritative audit chain.
                let audit_placeholder = self
                    .config
                    .paths
                    .state_dir
                    .join(".snapshot-audit-placeholder");
                if let Err(err) = std::fs::write(&audit_placeholder, b"") {
                    return Response {
                        status: ResponseStatus::InternalError,
                        message: format!("snapshot create: stage audit: {err}"),
                    };
                }
                let config_bytes: Vec<u8> =
                    serde_json::to_vec_pretty(&self.config).unwrap_or_else(|_| b"{}".to_vec());
                let sidecar = match snap::create_snapshot(
                    &store_path,
                    &vault_path,
                    &audit_placeholder,
                    &config_bytes,
                    &path,
                    &opts,
                ) {
                    Ok(s) => {
                        let _ = std::fs::remove_file(&audit_placeholder);
                        s
                    }
                    Err(err) => {
                        let _ = std::fs::remove_file(&audit_placeholder);
                        return snapshot_error_to_response("snapshot create", err);
                    }
                };
                let payload = serde_json::json!({
                    "archive": path.to_string_lossy(),
                    "sidecar": snap::sidecar_path_for(&path).to_string_lossy(),
                    "sha3_256": sidecar.sha3_256,
                    "zstd_level": sidecar.zstd_level,
                    "encrypted": sidecar.encrypted,
                    "size_bytes": sidecar.archive_size_bytes,
                });
                self.audited_response(
                    "snapshot.create",
                    Some(format!(
                        "sha3={} size={} encrypted={} zstd={}",
                        sidecar.sha3_256,
                        sidecar.archive_size_bytes,
                        sidecar.encrypted,
                        sidecar.zstd_level
                    )),
                    payload.to_string(),
                )
            }
            SnapshotAction::Verify => {
                let _ = (gpg_recipient, yes, retention_days, zstd_level);
                match snap::verify_snapshot(&path) {
                    Ok(sidecar) => {
                        let payload = serde_json::json!({
                            "ok": true,
                            "sha3_256": sidecar.sha3_256,
                            "encrypted": sidecar.encrypted,
                            "zstd_level": sidecar.zstd_level,
                        });
                        self.audited_response(
                            "snapshot.verify",
                            Some(format!("sha3={}", sidecar.sha3_256)),
                            payload.to_string(),
                        )
                    }
                    Err(err) => snapshot_error_to_response("snapshot verify", err),
                }
            }
            SnapshotAction::Restore => {
                if !yes {
                    return Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "snapshot restore is destructive: pass --yes".to_owned(),
                    };
                }
                let targets = snap::RestoreTargets {
                    target_dir: self.config.paths.state_dir.clone(),
                };
                match snap::restore_snapshot(&path, &targets) {
                    Ok(sidecar) => {
                        let payload = serde_json::json!({
                            "ok": true,
                            "target_dir": targets.target_dir.to_string_lossy(),
                            "sha3_256": sidecar.sha3_256,
                        });
                        self.audited_response(
                            "snapshot.restore",
                            Some(format!("sha3={}", sidecar.sha3_256)),
                            payload.to_string(),
                        )
                    }
                    Err(err) => snapshot_error_to_response("snapshot restore", err),
                }
            }
            SnapshotAction::Prune => {
                if !yes {
                    return Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "snapshot prune is destructive: pass --yes".to_owned(),
                    };
                }
                let Some(days) = retention_days else {
                    return Response {
                        status: ResponseStatus::InvalidRequest,
                        message: "snapshot prune requires --retention-days".to_owned(),
                    };
                };
                match snap::prune_gfs_execute(&path, days) {
                    Ok(removed) => {
                        let removed_strs: Vec<String> = removed
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect();
                        let payload = serde_json::json!({
                            "ok": true,
                            "removed_count": removed.len(),
                            "removed": removed_strs,
                        });
                        self.audited_response(
                            "snapshot.prune",
                            Some(format!("removed={} retention_days={}", removed.len(), days)),
                            payload.to_string(),
                        )
                    }
                    Err(err) => snapshot_error_to_response("snapshot prune", err),
                }
            }
            _ => Response {
                status: ResponseStatus::InvalidRequest,
                message: "snapshot: unsupported action".to_owned(),
            },
        }
    }

    /// H14 PR4 — TODO(bd-1du.4.6.1): bootstrap caller for the integrity
    /// sweeper worker thread. Currently a no-op stub. The caller in
    /// `bootstrap.rs` should invoke this once the runtime is fully
    /// assembled so the worker can take a clone of the audit-emission
    /// callback. Left as a follow-up because spawning a thread that
    /// borrows `&mut self` requires a small refactor of the audit sink
    /// to flow through an `Arc<Mutex<StoreProfile>>` shim. Until that
    /// lands, IPC `IntegrityRunOnce` calls execute synchronously on the
    /// dispatch thread, which is sufficient for PR4's plumbing scope.
    pub fn bootstrap_integrity_sweeper(&mut self) {
        // Intentionally empty. See above.
    }

    fn audited_response(
        &mut self,
        category: impl Into<String>,
        details: Option<String>,
        success_message: impl Into<String>,
    ) -> Response {
        let success_message = success_message.into();
        match self.record_audit_event(category, details) {
            Ok(()) => Response {
                status: ResponseStatus::Ok,
                message: success_message,
            },
            Err(err) => Response {
                status: ResponseStatus::InternalError,
                message: format!("{success_message}; audit persistence failed: {err}"),
            },
        }
    }

    /// Evaluate the `[data_residency]` allow-list for a single call site.
    ///
    /// Public so integration tests in `tests/residency.rs` can cover
    /// each enforcement call site without requiring a live pCloud
    /// session. Production dispatch reaches this helper through the
    /// three call sites that pass in `ACTION_SYNC_ROOT_ADD`,
    /// `ACTION_UPLOAD_CREATE`, or a future action label.
    ///
    /// Returns an `Option<Response>`:
    ///
    /// - `None` — operation may proceed (policy unrestricted, region
    ///   permitted, or non-strict warn-only). In the warn-only case an
    ///   audit record is persisted with `residency.warn` so operators
    ///   can count near-misses without blocking traffic.
    /// - `Some(response)` — strict-mode refusal. The response carries
    ///   [`ResponseStatus::PolicyViolation`] with
    ///   `kind = "data_residency"` and a helpful message naming the
    ///   offending region and the configured allow-list. A
    ///   `residency.violation` audit record is persisted before return.
    ///
    /// `action` is copied verbatim into the audit event so the
    /// call-site label is stable across releases
    /// (`ACTION_SYNC_ROOT_ADD`, `ACTION_UPLOAD_CREATE`, etc.). `region`
    /// is resolved by the caller — typically from
    /// [`pcloud_backends::residency::resolve_region_from_host`]
    /// against either the active API host or the folder-metadata hint.
    ///
    /// Audit-persistence failures are not swallowed: they surface as
    /// `InternalError` in the refusal path; on the allow path, an
    /// eprintln warning is emitted and the operation proceeds (matches
    /// `audited_response` behaviour — the existing enterprise rule is
    /// "never silently drop an audit event on a control path"; an
    /// allow-with-warn is not a control path).
    pub fn check_residency(
        &mut self,
        action: &'static str,
        region: pcloud_backends::residency::Region,
    ) -> Option<Response> {
        let (decision, evt) =
            pcloud_backends::residency::enforce(&self.config.data_residency, region, action);
        // Build the audit detail payload once; reused by both branches
        // and by the final release/rejection path.
        let details = format!(
            "op={} region={} allowed={:?} refused={} warned={}",
            evt.action,
            evt.region.as_tag(),
            evt.allowed,
            evt.refused,
            evt.warned,
        );
        match decision {
            pcloud_backends::residency::ResidencyDecision::Allow => {
                if evt.warned {
                    // Warn-only: still log a durable audit entry so
                    // operators can grep for `residency.warn` and
                    // count near-misses pre-strict rollout.
                    if let Err(err) =
                        self.record_audit_event("residency.warn", Some(details.clone()))
                    {
                        log::warn!("pcloud-rs: residency warn-audit failed for {action}: {err}");
                    }
                }
                None
            }
            pcloud_backends::residency::ResidencyDecision::Refuse => {
                // Persist a hard-refusal audit event before emitting
                // the wire error; audit chain is the source of truth.
                let audit_err = self
                    .record_audit_event("residency.violation", Some(details.clone()))
                    .err();
                let message = format!(
                    "data-residency policy refused {op}: region {region} is not in \
                     allowed_regions={allowed:?} (strict mode)",
                    op = evt.action,
                    region = evt.region.as_tag(),
                    allowed = evt.allowed,
                );
                if let Some(err) = audit_err {
                    return Some(Response {
                        status: ResponseStatus::InternalError,
                        message: format!("{message}; residency audit persistence failed: {err}"),
                    });
                }
                Some(Response {
                    status: ResponseStatus::PolicyViolation {
                        kind: pcloud_backends::residency::POLICY_KIND_DATA_RESIDENCY.to_owned(),
                    },
                    message,
                })
            }
        }
    }

    /// Resolve the active API-host region, memoizing through
    /// `self.residency_cache` keyed by a stable synthetic id
    /// (`u64::MAX` — the API host is a singleton per daemon session, so
    /// a fixed key is sufficient and avoids folder-id collisions). The
    /// cache lives here rather than inside `pcloud-backends` so tests
    /// can assert on hit counts; production daemons rebuild on restart.
    fn active_host_region(&self) -> pcloud_backends::residency::Region {
        const HOST_CACHE_KEY: u64 = u64::MAX;
        let host = self.config.api.host.clone();
        self.residency_cache
            .resolve_or_insert_with(HOST_CACHE_KEY, move || {
                pcloud_backends::residency::resolve_region_from_host(&host)
            })
    }

    fn persist_auth_state(&mut self) -> Result<(), PersistAuthStateError> {
        let previous_account = self.store.repositories.accounts.primary_account.clone();
        let vault_path = self.config.paths.auth_token_vault_path();
        let previous_vault_token = load_token(&vault_path)?;

        self.sync_auth_vault()?;

        if let Err(err) = self.sync_account_store() {
            self.store.repositories.accounts.primary_account = previous_account;
            self.restore_vault_state(previous_vault_token)?;
            return Err(PersistAuthStateError::Store(err));
        }

        Ok(())
    }

    fn sync_account_store(&mut self) -> Result<(), pcloud_store::StoreError> {
        self.store.repositories.accounts.primary_account = self.desired_account_record();
        persist_profile(&self.store)
    }

    fn sync_auth_vault(&self) -> Result<(), AuthVaultError> {
        if !self.config.features.durable_auth_tokens_enabled {
            return clear_token(&self.config.paths.auth_token_vault_path());
        }
        let path = self.config.paths.auth_token_vault_path();
        match self.auth.snapshot().auth_token.as_ref() {
            Some(token) => store_token(&path, token),
            None => clear_token(&path),
        }
    }

    fn restore_vault_state(
        &self,
        previous_vault_token: Option<SecretString>,
    ) -> Result<(), AuthVaultError> {
        if !self.config.features.durable_auth_tokens_enabled {
            return clear_token(&self.config.paths.auth_token_vault_path());
        }
        let path = self.config.paths.auth_token_vault_path();
        match previous_vault_token.as_ref() {
            Some(token) => store_token(&path, token),
            None => clear_token(&path),
        }
    }

    fn desired_account_record(&self) -> Option<AccountRecord> {
        let snapshot = self.auth.snapshot();
        match (snapshot.authenticated_user, snapshot.email.as_ref()) {
            (Some(user_id), Some(email)) => Some(AccountRecord {
                user_id,
                email: email.clone(),
                auth_token_present: snapshot.auth_token.is_some(),
            }),
            _ => None,
        }
    }

    fn read_local_upload_payload(
        &mut self,
        path: &str,
    ) -> Result<Vec<u8>, pcloud_engine::recovery::RecoveryFailure> {
        if let Ok(result) = self.filesystem.read_staged_path(path, 0, usize::MAX) {
            return Ok(result.bytes);
        }

        if let Some(bytes) = self.cache.staging.get(path) {
            return Ok(bytes.to_vec());
        }

        Err(pcloud_engine::recovery::RecoveryFailure::InvalidPath)
    }

    // --- Shares / business / teams ---

    fn shares_require_auth_token(&self, purpose: &str) -> Result<SecretString, Response> {
        match self.auth.snapshot().auth_token.as_ref() {
            Some(token) => Ok(token.clone_secret()),
            None => Err(Response {
                status: ResponseStatus::Conflict,
                message: format!("{purpose} requires an authenticated session"),
            }),
        }
    }

    fn list_shares(&mut self, incoming: bool) -> Response {
        let token = match self.shares_require_auth_token("listing shares") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self.shares_runtime.list_shares(token, incoming) {
            Ok(shares) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "shares: direction={}, count={}, ids={:?}",
                    if incoming { "incoming" } else { "outgoing" },
                    shares.len(),
                    shares.iter().map(|s| s.share_id).collect::<Vec<_>>()
                ),
            },
            Err(err) => map_shares_error(err),
        }
    }

    fn list_share_requests(&mut self, incoming: bool) -> Response {
        let token = match self.shares_require_auth_token("listing share requests") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self.shares_runtime.list_share_requests(token, incoming) {
            Ok(reqs) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "share_requests: direction={}, count={}, ids={:?}",
                    if incoming { "incoming" } else { "outgoing" },
                    reqs.len(),
                    reqs.iter().map(|r| r.share_request_id).collect::<Vec<_>>()
                ),
            },
            Err(err) => map_shares_error(err),
        }
    }

    fn list_contacts(&mut self) -> Response {
        let token = match self.shares_require_auth_token("listing contacts") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self.shares_runtime.list_contacts(token) {
            Ok(contacts) => Response {
                status: ResponseStatus::Ok,
                message: format!("contacts: count={}", contacts.len()),
            },
            Err(err) => map_shares_error(err),
        }
    }

    fn list_my_teams(&mut self) -> Response {
        let token = match self.shares_require_auth_token("listing teams") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self.shares_runtime.list_my_teams(token) {
            Ok(teams) => Response {
                status: ResponseStatus::Ok,
                message: format!(
                    "myteams: count={}, ids={:?}",
                    teams.len(),
                    teams.iter().map(|t| t.team_id).collect::<Vec<_>>()
                ),
            },
            Err(err) => map_shares_error(err),
        }
    }

    fn share_folder(
        &mut self,
        folder_id: u64,
        name: String,
        mail: String,
        message: String,
        permissions_bits: u32,
        hint: Option<String>,
    ) -> Response {
        if name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "share name must not be empty".to_owned(),
            };
        }
        if !mail.contains('@') {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "share recipient mail must be a valid address".to_owned(),
            };
        }
        let token = match self.shares_require_auth_token("share folder") {
            Ok(t) => t,
            Err(r) => return r,
        };
        let perms = SharePermissions::from_bits(permissions_bits);
        match self.shares_runtime.share_folder(
            token,
            folder_id,
            name,
            mail.clone(),
            message,
            perms,
            hint,
        ) {
            Ok(out) => self.audited_response(
                "shares.create",
                Some(format!(
                    "folder_id={folder_id} mail={mail} request_id={:?}",
                    out.share_request_id
                )),
                format!(
                    "share request sent: folder_id={folder_id}, sharerequestid={:?}",
                    out.share_request_id
                ),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn cancel_share_request(&mut self, share_request_id: u64) -> Response {
        let token = match self.shares_require_auth_token("cancel share request") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self
            .shares_runtime
            .cancel_share_request(token, share_request_id)
        {
            Ok(()) => self.audited_response(
                "shares.cancel",
                Some(format!("sharerequestid={share_request_id}")),
                format!("share request cancelled: id={share_request_id}"),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn decline_share_request(&mut self, share_request_id: u64) -> Response {
        let token = match self.shares_require_auth_token("decline share request") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self
            .shares_runtime
            .decline_share_request(token, share_request_id)
        {
            Ok(()) => self.audited_response(
                "shares.decline",
                Some(format!("sharerequestid={share_request_id}")),
                format!("share request declined: id={share_request_id}"),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn accept_share_request(
        &mut self,
        share_request_id: u64,
        to_folder_id: u64,
        name: Option<String>,
    ) -> Response {
        let token = match self.shares_require_auth_token("accept share request") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self.shares_runtime.accept_share_request(
            token,
            share_request_id,
            to_folder_id,
            name.clone(),
        ) {
            Ok(()) => self.audited_response(
                "shares.accept",
                Some(format!(
                    "sharerequestid={share_request_id} tofolderid={to_folder_id} name={:?}",
                    name
                )),
                format!(
                    "share request accepted: id={share_request_id}, to_folder_id={to_folder_id}"
                ),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn remove_share(&mut self, share_id: u64) -> Response {
        let token = match self.shares_require_auth_token("remove share") {
            Ok(t) => t,
            Err(r) => return r,
        };
        match self.shares_runtime.remove_share(token, share_id) {
            Ok(()) => self.audited_response(
                "shares.remove",
                Some(format!("shareid={share_id}")),
                format!("share removed: id={share_id}"),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn modify_share(&mut self, share_id: u64, permissions_bits: u32) -> Response {
        let token = match self.shares_require_auth_token("modify share") {
            Ok(t) => t,
            Err(r) => return r,
        };
        let perms = SharePermissions::from_bits(permissions_bits);
        match self.shares_runtime.modify_share(token, share_id, perms) {
            Ok(()) => self.audited_response(
                "shares.modify",
                Some(format!(
                    "shareid={share_id} permissions={}",
                    perms.to_bits()
                )),
                format!(
                    "share permissions updated: id={share_id}, permissions={}",
                    perms.to_bits()
                ),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn account_stop_share(
        &mut self,
        user_share_ids: Vec<u64>,
        team_share_ids: Vec<u64>,
    ) -> Response {
        if user_share_ids.is_empty() && team_share_ids.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "account_stopshare requires at least one share id".to_owned(),
            };
        }
        let token = match self.shares_require_auth_token("account stop share") {
            Ok(t) => t,
            Err(r) => return r,
        };
        let u_len = user_share_ids.len();
        let t_len = team_share_ids.len();
        match self
            .shares_runtime
            .account_stop_share(token, user_share_ids, team_share_ids)
        {
            Ok(()) => self.audited_response(
                "shares.account_stop",
                Some(format!("users={u_len} teams={t_len}")),
                format!("account_stopshare ok: users={u_len}, teams={t_len}"),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn account_modify_share(
        &mut self,
        user_shares: Vec<(u64, u32)>,
        team_shares: Vec<(u64, u32)>,
    ) -> Response {
        if user_shares.is_empty() && team_shares.is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "account_modifyshare requires at least one share id".to_owned(),
            };
        }
        let token = match self.shares_require_auth_token("account modify share") {
            Ok(t) => t,
            Err(r) => return r,
        };
        let u: Vec<(u64, SharePermissions)> = user_shares
            .into_iter()
            .map(|(id, bits)| (id, SharePermissions::from_bits(bits)))
            .collect();
        let t: Vec<(u64, SharePermissions)> = team_shares
            .into_iter()
            .map(|(id, bits)| (id, SharePermissions::from_bits(bits)))
            .collect();
        let u_len = u.len();
        let t_len = t.len();
        match self.shares_runtime.account_modify_share(token, u, t) {
            Ok(()) => self.audited_response(
                "shares.account_modify",
                Some(format!("users={u_len} teams={t_len}")),
                format!("account_modifyshare ok: users={u_len}, teams={t_len}"),
            ),
            Err(err) => map_shares_error(err),
        }
    }

    fn account_team_share(
        &mut self,
        folder_id: u64,
        name: String,
        team_id: u64,
        message: String,
        permissions_bits: u32,
        hint: Option<String>,
    ) -> Response {
        if name.trim().is_empty() {
            return Response {
                status: ResponseStatus::InvalidRequest,
                message: "team share name must not be empty".to_owned(),
            };
        }
        let token = match self.shares_require_auth_token("account team share") {
            Ok(t) => t,
            Err(r) => return r,
        };
        let perms = SharePermissions::from_bits(permissions_bits);
        match self.shares_runtime.account_team_share(
            token,
            folder_id,
            name,
            team_id,
            message,
            perms,
            hint,
        ) {
            Ok(out) => self.audited_response(
                "shares.account_team_share",
                Some(format!(
                    "folder_id={folder_id} team_id={team_id} request_id={:?}",
                    out.share_request_id
                )),
                format!(
                    "team share sent: folder_id={folder_id}, team_id={team_id}, sharerequestid={:?}",
                    out.share_request_id
                ),
            ),
            Err(err) => map_shares_error(err),
        }
    }
}

/// Map a [`pcloud_proto::revision_provider::RevisionError`] to a
/// [`ResponseStatus`]. Null/not-configured, transport, HTTP-status, and
/// malformed-response errors map to `Unavailable` so the CLI exits 6
/// with an actionable hint; malformed requests map to `InvalidRequest`
/// so the caller corrects the CLI invocation.
fn revision_err_to_status(err: &pcloud_proto::revision_provider::RevisionError) -> ResponseStatus {
    use pcloud_proto::revision_provider::RevisionError;
    match err {
        RevisionError::InvalidRequest(_) => ResponseStatus::InvalidRequest,
        _ => ResponseStatus::Unavailable,
    }
}

/// Render a [`pcloud_proto::revision_provider::RevisionError`] as a
/// structured JSON payload embedded in [`Response::message`].
///
/// Shape:
///
/// - `status` → short error kind (`"not_configured"`, `"invalid_url"`,
///   `"transport"`, `"http_status"`, `"malformed_response"`, `"invalid_request"`).
/// - `message` → the human-readable error message.
/// - `next` → actionable remediation hint where one applies.
/// - `path` → the path that triggered the error, for audit correlation.
///
/// Tooling keyed on the `status` field gets a stable taxonomy.
fn render_revision_error_payload(
    err: &pcloud_proto::revision_provider::RevisionError,
    path: &str,
) -> String {
    use pcloud_proto::revision_provider::RevisionError;
    let (kind, next): (&str, &str) = match err {
        RevisionError::NotConfigured(_) => (
            "not_configured",
            "configure [file_history].revision_url or wait for pCloud public API",
        ),
        RevisionError::InvalidUrl(_) => (
            "invalid_url",
            "fix [file_history].revision_url: must be https:// (http:// only in dev)",
        ),
        RevisionError::Transport(_) => (
            "transport",
            "verify network reachability to [file_history].revision_url",
        ),
        RevisionError::HttpStatus { .. } => (
            "http_status",
            "inspect the revision endpoint logs for non-2xx responses",
        ),
        RevisionError::MalformedResponse(_) => (
            "malformed_response",
            "ensure the endpoint returns a JSON array of revisions or {\"revisions\":[...]}",
        ),
        RevisionError::InvalidRequest(_) => (
            "invalid_request",
            "pass a non-empty absolute remote path (e.g. /Docs/report.txt)",
        ),
    };
    let payload = serde_json::json!({
        "status": kind,
        "message": err.to_string(),
        "next": next,
        "path": path,
    });
    serde_json::to_string(&payload)
        .unwrap_or_else(|_| format!("{{\"status\":\"{kind}\",\"message\":\"{err}\"}}"))
}

fn map_shares_error(err: pcloud_proto::SharesApiError<SharesBackendError>) -> Response {
    match err {
        pcloud_proto::SharesApiError::Result { message, .. } => Response {
            status: ResponseStatus::Conflict,
            message: message.unwrap_or_else(|| "share request failed".to_owned()),
        },
        other => Response {
            status: ResponseStatus::Unavailable,
            message: other.to_string(),
        },
    }
}

/// Collapse a [`pcloud_backends::snapshot::SnapshotError`] into a
/// [`Response`]. Operator-friendly invariant violations
/// (InvalidZstdLevel, InvalidOutputSuffix, SidecarMissing) surface as
/// `InvalidRequest`; integrity failures surface as `Conflict`;
/// everything else collapses into `InternalError`.
fn snapshot_error_to_response(op: &str, err: pcloud_backends::snapshot::SnapshotError) -> Response {
    use pcloud_backends::snapshot::SnapshotError as S;
    let status = match err {
        S::InvalidZstdLevel { .. } | S::InvalidOutputSuffix => ResponseStatus::InvalidRequest,
        S::DigestMismatch | S::SchemaMismatch { .. } => ResponseStatus::Conflict,
        S::SidecarMissing | S::SidecarCorrupt => ResponseStatus::InvalidRequest,
        S::GpgUnavailable | S::GpgRecipientMissing | S::GpgFailed => ResponseStatus::Unavailable,
        _ => ResponseStatus::InternalError,
    };
    Response {
        status,
        message: format!("{op}: {err}"),
    }
}

fn sync_root_path_conflict(candidate: &Path, existing: &str) -> Option<String> {
    let existing_path = PathBuf::from(existing);
    if candidate == existing_path {
        return Some(format!(
            "local sync root is already tracked: {}",
            existing_path.display()
        ));
    }
    if candidate.starts_with(&existing_path) {
        return Some(format!(
            "local sync root is inside an already tracked sync root: {}",
            existing_path.display()
        ));
    }
    if existing_path.starts_with(candidate) {
        return Some(format!(
            "local sync root would contain an already tracked sync root: {}",
            existing_path.display()
        ));
    }
    None
}

#[derive(Debug, thiserror::Error)]
enum PersistAuthStateError {
    #[error("store persistence failed: {0}")]
    Store(#[from] pcloud_store::StoreError),
    #[error("vault persistence failed: {0}")]
    Vault(#[from] AuthVaultError),
}

fn map_auth_flow_error(err: pcloud_auth::AuthFlowError<AuthBackendError>) -> Response {
    match err {
        pcloud_auth::AuthFlowError::Session(session_err) => Response {
            status: ResponseStatus::Conflict,
            message: session_err.to_string(),
        },
        pcloud_auth::AuthFlowError::Protocol(protocol_err) => Response {
            status: ResponseStatus::Unavailable,
            message: protocol_err.to_string(),
        },
    }
}

fn map_notifications_error(
    err: pcloud_proto::NotificationsApiError<NotificationsBackendError>,
) -> Response {
    match err {
        pcloud_proto::NotificationsApiError::Result { message, .. } => Response {
            status: ResponseStatus::Conflict,
            message: message.unwrap_or_else(|| "notifications request failed".to_owned()),
        },
        other => Response {
            status: ResponseStatus::Unavailable,
            message: other.to_string(),
        },
    }
}

fn map_folder_error(err: pcloud_proto::FolderApiError<FolderBackendError>) -> Response {
    match err {
        pcloud_proto::FolderApiError::Result { message, .. } => Response {
            status: ResponseStatus::Conflict,
            message: message.unwrap_or_else(|| "createfolder request failed".to_owned()),
        },
        other => Response {
            status: ResponseStatus::Unavailable,
            message: other.to_string(),
        },
    }
}

/// Map a [`crate::path_resolver::PathResolveError`] into a
/// [`Response`] using the same policy the other feature-backend mappers
/// apply:
/// - `InvalidPath` / `ExpectedFolder` / `ExpectedFile` -> `InvalidRequest`,
/// - `NotFound` / `Ambiguous` / `MissingId` -> `Conflict`,
/// - `Transport` -> `Unavailable`.
///
/// The error `Display` output is safe to surface: it never contains the
/// auth token — the resolver only ever borrows it for the duration of
/// the `listfolder` call.
fn map_path_resolve_error(err: crate::path_resolver::PathResolveError) -> Response {
    use crate::path_resolver::PathResolveError as E;
    match &err {
        E::InvalidPath { .. } | E::ExpectedFolder { .. } | E::ExpectedFile { .. } => Response {
            status: ResponseStatus::InvalidRequest,
            message: err.to_string(),
        },
        E::NotFound { .. } | E::Ambiguous { .. } | E::MissingId { .. } => Response {
            status: ResponseStatus::Conflict,
            message: err.to_string(),
        },
        E::Transport { .. } => Response {
            status: ResponseStatus::Unavailable,
            message: err.to_string(),
        },
    }
}

/// Derive a pCloud share short-code from a full public-link URL.
///
/// The pCloud public link shape is
/// `https://<host>/<CODE>` (or `publink/show?code=<CODE>` for the
/// long form). The backend model carries the full URL but does not
/// expose the short-code as a separate field on
/// [`pcloud_model::public_links::CreatedPublicLink`] or
/// [`pcloud_model::public_links::CreatedUploadLink`]. We scrape it
/// from the trailing path segment / query argument so the CLI
/// `--json` recipes can pipe `.code` through `jq` without regex.
/// Returns an empty string when no code can be extracted (the JSON
/// field is still emitted — never dropped — so the schema stays
/// stable).
/// Split an absolute pCloud-drive `path` into `(parent, leaf)`, where
/// `parent` is the absolute path of the directory holding the entry
/// and `leaf` is the basename. Returns `None` for inputs that have
/// no leaf to extract (the empty string, the root `/`, or a path
/// that's all slashes).
///
/// Used by [`Runtime::rename_path`] (bd-smbr-pcloud P4.3) to derive
/// the destination's parent + new name from a single `to` path
/// argument.
fn split_parent_and_leaf(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let split = trimmed.rsplit_once('/')?;
    let parent = if split.0.is_empty() {
        "/".to_owned()
    } else {
        split.0.to_owned()
    };
    let leaf = split.1.to_owned();
    if leaf.is_empty() {
        return None;
    }
    Some((parent, leaf))
}

fn short_code_from_link(link: &str) -> String {
    // Prefer the explicit `code=` form when present (webapp links).
    if let Some((_, tail)) = link.split_once("code=") {
        let end = tail.find(['&', '#']).unwrap_or(tail.len());
        return tail[..end].to_owned();
    }
    // Otherwise take the last non-empty path segment.
    let trimmed = link.trim_end_matches('/');
    let Some(seg) = trimmed.rsplit('/').next() else {
        return String::new();
    };
    let end = seg.find(['?', '#']).unwrap_or(seg.len());
    seg[..end].to_owned()
}

fn map_public_link_error(
    err: pcloud_proto::PublicLinksApiError<PublicLinkBackendError>,
) -> Response {
    match err {
        pcloud_proto::PublicLinksApiError::Result { message, .. } => Response {
            status: ResponseStatus::Conflict,
            message: message.unwrap_or_else(|| "public link request failed".to_owned()),
        },
        other => Response {
            status: ResponseStatus::Unavailable,
            message: other.to_string(),
        },
    }
}

fn resolve_upload_parent_folder_id(
    path: &str,
    remote_parent_folder_id: Option<pcloud_model::ids::RemoteFolderId>,
) -> Result<u64, pcloud_engine::recovery::RecoveryFailure> {
    let parent = path
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("");
    if parent.is_empty() {
        return Ok(0);
    }
    remote_parent_folder_id
        .map(|id| id.get())
        .ok_or(pcloud_engine::recovery::RecoveryFailure::InvalidPath)
}

// ==========================================================================
// Observability helpers (feature = "metrics")
//
// All helpers are pure + allocate-free. Method labels are `&'static str`
// constants derived from the `Request` variant, so there is no possibility
// of label explosion or accidental secret leakage via Debug formatting. The
// upstream `pcloud_observability::metrics::sanitize_label` still applies a
// defensive pass on the other end of the call.
// ==========================================================================

#[cfg(feature = "metrics")]
pub(crate) fn method_label(request: &Request) -> &'static str {
    match request {
        Request::Plain { method } => match method {
            pcloud_ipc::Method::GetStatus => "GetStatus",
            pcloud_ipc::Method::GetHealth => "GetHealth",
            pcloud_ipc::Method::Health => "Health",
            pcloud_ipc::Method::GetPending => "GetPending",
            pcloud_ipc::Method::GetSyncRoots => "GetSyncRoots",
            pcloud_ipc::Method::ListPublicLinks => "ListPublicLinks",
            pcloud_ipc::Method::ListUploadLinks => "ListUploadLinks",
            pcloud_ipc::Method::GetUserInfo => "GetUserInfo",
            pcloud_ipc::Method::PauseSync => "PauseSync",
            pcloud_ipc::Method::ResumeSync => "ResumeSync",
            pcloud_ipc::Method::LoginBegin => "LoginBegin",
            pcloud_ipc::Method::Logout => "Logout",
            pcloud_ipc::Method::SendTwoFactorSms => "SendTwoFactorSms",
            pcloud_ipc::Method::SendTwoFactorNotification => "SendTwoFactorNotification",
            pcloud_ipc::Method::SubmitPassword => "SubmitPassword",
            pcloud_ipc::Method::SubmitTwoFactorCode => "SubmitTwoFactorCode",
            pcloud_ipc::Method::UnlockCrypto => "UnlockCrypto",
            pcloud_ipc::Method::LockCrypto => "LockCrypto",
            pcloud_ipc::Method::GetCryptoStatus => "GetCryptoStatus",
            pcloud_ipc::Method::CryptoReset => "CryptoReset",
            pcloud_ipc::Method::GetCryptoPrivKeyFlags => "GetCryptoPrivKeyFlags",
            pcloud_ipc::Method::SendCryptoChangeUserPrivate => "SendCryptoChangeUserPrivate",
            pcloud_ipc::Method::Shutdown => "Shutdown",
            pcloud_ipc::Method::SetAuthPersistence => "SetAuthPersistence",
            pcloud_ipc::Method::ListIncomingShares => "ListIncomingShares",
            pcloud_ipc::Method::ListOutgoingShares => "ListOutgoingShares",
            pcloud_ipc::Method::ListIncomingShareRequests => "ListIncomingShareRequests",
            pcloud_ipc::Method::ListOutgoingShareRequests => "ListOutgoingShareRequests",
            pcloud_ipc::Method::ListContacts => "ListContacts",
            pcloud_ipc::Method::ListMyTeams => "ListMyTeams",
            pcloud_ipc::Method::ListNotifications => "ListNotifications",
            pcloud_ipc::Method::GetSlo => "GetSlo",
            pcloud_ipc::Method::DrainStatus => "DrainStatus",
            pcloud_ipc::Method::GetAuditVerifierStatus => "GetAuditVerifierStatus",
            pcloud_ipc::Method::GetSyncStatus => "GetSyncStatus",
            pcloud_ipc::Method::ListConflicts => "ListConflicts",
            pcloud_ipc::Method::GetApiServers => "GetApiServers",
            pcloud_ipc::Method::GetPromo => "GetPromo",
            pcloud_ipc::Method::GetCryptoHint => "GetCryptoHint",
            pcloud_ipc::Method::VerifyEmail => "VerifyEmail",
            _ => "Other",
        },
        Request::PasswordSubmission { .. } => "PasswordSubmission",
        Request::AuthTokenSubmission { .. } => "AuthTokenSubmission",
        Request::TwoFactorCodeSubmission { .. } => "TwoFactorCodeSubmission",
        Request::CryptoUnlock { .. } => "CryptoUnlock",
        Request::CryptoSetup { .. } => "CryptoSetup",
        Request::CryptoSetupV2 { .. } => "crypto_setuserkeys",
        Request::CryptoGetFolderKey { .. } => "crypto_getfolderkey",
        Request::CryptoGetFileKey { .. } => "crypto_getfilekey",
        Request::CryptoMkdir { .. } => "CryptoMkdir",
        Request::CryptoChangePassword { .. } => "CryptoChangePassword",
        Request::CryptoChangePasswordUnlocked { .. } => "CryptoChangePasswordUnlocked",
        Request::AuthPersistence { .. } => "AuthPersistence",
        Request::SyncRootAdd { .. } => "SyncRootAdd",
        Request::SyncRootRemove { .. } => "SyncRootRemove",
        Request::SyncRootPause { .. } => "SyncRootPause",
        Request::SyncRootResume { .. } => "SyncRootResume",
        Request::SyncRootChangeType { .. } => "SyncRootChangeType",
        Request::GetSyncSuggestions { .. } => "GetSyncSuggestions",
        Request::IsFolderSyncable { .. } => "IsFolderSyncable",
        Request::ShowPublicLink { .. } => "ShowPublicLink",
        Request::DeletePublicLink { .. } => "DeletePublicLink",
        Request::DeletePublicLinkByCode { .. } => "DeletePublicLinkByCode",
        Request::CreateFilePublicLink { .. } => "CreateFilePublicLink",
        Request::CreateFolderPublicLink { .. } => "CreateFolderPublicLink",
        Request::ChangePublicLinkExpire { .. } => "ChangePublicLinkExpire",
        Request::ChangePublicLinkPassword { .. } => "ChangePublicLinkPassword",
        Request::ChangePublicLinkUpload { .. } => "ChangePublicLinkUpload",
        Request::CreateUploadLink { .. } => "CreateUploadLink",
        Request::DeleteUploadLink { .. } => "DeleteUploadLink",
        Request::CreateTreePublicLink { .. } => "CreateTreePublicLink",
        Request::ListPublicLinkAccess { .. } => "ListPublicLinkAccess",
        Request::AddPublicLinkAccess { .. } => "AddPublicLinkAccess",
        Request::RemovePublicLinkAccess { .. } => "RemovePublicLinkAccess",
        Request::ListBookmarks => "ListBookmarks",
        Request::RemoveBookmark { .. } => "RemoveBookmark",
        Request::ChangeBookmark { .. } => "ChangeBookmark",
        Request::ShareFolder { .. } => "ShareFolder",
        Request::CancelShareRequest { .. } => "CancelShareRequest",
        Request::DeclineShareRequest { .. } => "DeclineShareRequest",
        Request::AcceptShareRequest { .. } => "AcceptShareRequest",
        Request::RemoveShare { .. } => "RemoveShare",
        Request::ModifyShare { .. } => "ModifyShare",
        Request::AccountStopShare { .. } => "AccountStopShare",
        Request::AccountModifyShare { .. } => "AccountModifyShare",
        Request::AccountTeamShare { .. } => "AccountTeamShare",
        Request::ValueGet { .. } => "ValueGet",
        Request::ValueSet { .. } => "ValueSet",
        Request::ValueHas { .. } => "ValueHas",
        Request::MarkNotificationsRead { .. } => "MarkNotificationsRead",
        Request::AuditVerifyChain { .. } => "AuditVerifyChain",
        Request::Mount { .. } => "Mount",
        Request::Unmount => "Unmount",
        Request::MountForceUnmount { .. } => "MountForceUnmount",
        Request::CreateRemoteFolder { .. } => "CreateRemoteFolder",
        Request::SessionStatus => "SessionStatus",
        Request::RunLocalScan => "RunLocalScan",
        Request::SendPublink { .. } => "SendPublink",
        Request::GetFolderIdByPath { .. } => "GetFolderIdByPath",
        Request::GetFolderFlags { .. } => "GetFolderFlags",
        Request::GetFolderOwnerId { .. } => "GetFolderOwnerId",
        Request::FilesystemStatus { .. } => "FilesystemStatus",
        Request::StatPath { .. } => "StatPath",
        Request::ListFolderByPath { .. } => "ListFolderByPath",
        Request::FileDeleteByPath { .. } => "FileDeleteByPath",
        Request::FolderDeleteByPath { .. } => "FolderDeleteByPath",
        Request::FolderDeleteById { .. } => "FolderDeleteById",
        Request::RenamePath { .. } => "RenamePath",
        Request::FileHistory { .. } => "FileHistory",
        Request::ConflictList => "ConflictList",
        Request::ConflictResolve { .. } => "ConflictResolve",
        Request::LostPassword { .. } => "LostPassword",
        Request::VerifyEmailRestricted { .. } => "VerifyEmailRestricted",
        Request::AccountChangePassword { .. } => "AccountChangePassword",
        Request::AccountRegister { .. } => "AccountRegister",
        Request::GetFileLink { .. } => "GetFileLink",
        Request::DownloadFile { .. } => "DownloadFile",
        Request::DeleteBackup { .. } => "DeleteBackup",
        Request::SetApiServer { .. } => "SetApiServer",
        Request::SetLanguage { .. } => "SetLanguage",
        Request::UploadWriteFromFile { .. } => "UploadWriteFromFile",
        Request::CreateTreePublicLinkFromPaths { .. } => "CreateTreePublicLinkFromPaths",
        Request::CreateBackup { .. } => "CreateBackup",
        Request::StopDevice { .. } => "StopDevice",
        Request::DeleteBackupDevice => "DeleteBackupDevice",
        _ => "Other",
    }
}

#[cfg(feature = "metrics")]
pub(crate) fn status_label(status: &ResponseStatus) -> &'static str {
    match status {
        ResponseStatus::Ok => "ok",
        ResponseStatus::InvalidRequest => "invalid_request",
        ResponseStatus::Unauthorized => "unauthorized",
        ResponseStatus::Conflict => "conflict",
        ResponseStatus::Unavailable => "unavailable",
        ResponseStatus::InternalError => "error",
        ResponseStatus::PolicyViolation { .. } => "policy_violation",
        _ => "unknown",
    }
}

#[cfg(feature = "metrics")]
pub(crate) fn auth_result_from_event(event: &pcloud_auth::AuthEvent) -> Option<AuthResult> {
    match event {
        pcloud_auth::AuthEvent::LoginSucceeded { .. } => Some(AuthResult::Success),
        pcloud_auth::AuthEvent::LoginFailed { .. } => Some(AuthResult::Failure),
        pcloud_auth::AuthEvent::TwoFactorChallengeIssued => Some(AuthResult::TwoFactorRequired),
        _ => None,
    }
}

#[cfg(feature = "metrics")]
pub(crate) fn crypto_state_label(state: pcloud_crypto::state::UnlockState) -> CryptoLockState {
    use pcloud_crypto::state::UnlockState;
    match state {
        UnlockState::NotSetup => CryptoLockState::Unsetup,
        UnlockState::Locked | UnlockState::Unlocking => CryptoLockState::Locked,
        UnlockState::Unlocked => CryptoLockState::Unlocked,
    }
}

/// Build the JSON payload for [`pcloud_ipc::Method::DrainStatus`].
///
/// Pulls the current drain state, in-flight counter, and elapsed drain
/// time out of the crate's [`crate::signals`] atomics and serialises
/// them into the stable [`pcloud_ipc::DrainStatusPayload`] envelope.
/// Called from the synchronous dispatch path; does not mutate any
/// shell state. Safe to call at any drain state — during `Running` the
/// payload reports `in_flight >= 1` (the call itself), `elapsed_drain_ms
/// = 0`; during `Draining` the elapsed time grows monotonically.
pub(crate) fn drain_status_response() -> Response {
    let payload = pcloud_ipc::DrainStatusPayload {
        state: crate::signals::drain_state().as_str().to_owned(),
        in_flight: crate::signals::in_flight(),
        elapsed_drain_ms: crate::signals::elapsed_drain_ms(),
    };
    match serde_json::to_string(&payload) {
        Ok(body) => Response {
            status: ResponseStatus::Ok,
            message: body,
        },
        Err(err) => Response {
            status: ResponseStatus::InternalError,
            message: format!("drain status encode failed: {err}"),
        },
    }
}

/// Install a process-wide panic hook that increments the crate-local
/// atomic panic counter. The counter can be folded into the live
/// `MetricFamilies` by calling [`RuntimeShell::refresh_panic_metric`] from
/// any active dispatch path. Safe to call multiple times; only the first
/// call installs a hook.
#[cfg(feature = "metrics")]
pub fn install_panic_metrics_hook() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANIC_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            prev(info);
        }));
    });
}

#[cfg(feature = "metrics")]
pub(crate) static PANIC_COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Short, stable label for a
/// [`pcloud_backends::upload_sessions::ConflictMode`]. Used in audit
/// trailers and in the JSON `conflict_mode` field of
/// `Request::UploadCreate` responses.
fn mode_label(mode: pcloud_backends::upload_sessions::ConflictMode) -> &'static str {
    use pcloud_backends::upload_sessions::ConflictMode;
    match mode {
        ConflictMode::Error => "error",
        ConflictMode::Overwrite => "overwrite",
        ConflictMode::Skip => "skip",
        ConflictMode::Rename => "rename",
    }
}

/// Shared body for `pause` / `resume` / `cancel` IPC responses. Audits
/// the transition on success and maps
/// [`pcloud_backends::upload_sessions::SessionError`] to the standard
/// response taxonomy:
///
/// * `NotFound`            → `ResponseStatus::InvalidRequest`
/// * `InvalidTransition`   → `ResponseStatus::Conflict`
///
/// Split into a free function so each handler stays a one-liner and
/// the borrow checker lets us take a `&mut` to the whole shell
/// through `f`.
fn upload_session_transition_response<F>(
    shell: &mut RuntimeShell,
    audit_category: &'static str,
    session_id: u64,
    f: F,
) -> Response
where
    F: FnOnce(
        &mut pcloud_backends::upload_sessions::SessionRegistry,
    ) -> Result<
        &pcloud_backends::upload_sessions::UploadSession,
        pcloud_backends::upload_sessions::SessionError,
    >,
{
    use pcloud_backends::upload_sessions::SessionError;
    match f(&mut shell.upload_sessions) {
        Ok(session) => {
            let payload = serde_json::to_string(session).unwrap_or_else(|_| "{}".to_owned());
            let state_label = session.state.label().to_owned();
            shell.audited_response(
                audit_category,
                Some(format!("session_id={session_id} state={state_label}")),
                payload,
            )
        }
        Err(SessionError::NotFound(_)) => Response {
            status: ResponseStatus::InvalidRequest,
            message: format!("upload session {session_id} not found"),
        },
        Err(err @ SessionError::InvalidTransition { .. }) => Response {
            status: ResponseStatus::Conflict,
            message: err.to_string(),
        },
    }
}

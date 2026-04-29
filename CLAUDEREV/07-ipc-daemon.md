# pcloud-rs Enterprise Readiness Audit — Dimension 7: IPC & Daemon

**Auditor:** Claude (read-only)
**Date:** 2026-04-29
**Scope:** `crates/pcloud-ipc/`, `crates/pcloud-daemon/src/`, `crates/pcloud-web/`,
plus the integration tests under each crate.
**Method:** Source review against the seven sub-checks in `pcloud_rev.md` §7.
No code or docs were modified.

---

## Summary

The IPC + daemon subsystem is one of the more disciplined parts of the
workspace. The wire format is bounded, length-prefixed, version-gated,
and decoded with an OOM cap before any allocation proportional to a
peer-controlled length prefix. Peer authentication is correctly wired
on every accepted connection on Linux (`SO_PEERCRED`), BSD/macOS
(`getpeereid(3)`), **and Windows** (`GetNamedPipeClientProcessId` +
`TokenUser` SID compare). The `CLAUDE.md` claim that the Windows
named-pipe accept loop is *not* wired through `serve_once_with_peer`
is **stale and contradicted by the source** — the dispatch is fully
plumbed and `serve_with_shutdown` runs the same loop on Windows as on
Unix. That's the single largest correction this dimension produces.

The graceful-drain machine, slow-client isolation, request-size cap,
per-peer connection cap, panic guard, and hot-reload path are all
real, exercised by tests, and consistent with the documented
contract. Property-based round-trip coverage of the `Request` /
`Response` schema is comprehensive (~70 variants, including the
recently-added `CryptoSetupV2`, `CryptoGetFolderKey`,
`CryptoGetFileKey`, `UploadWriteFromFile`,
`CreateTreePublicLinkFromPaths`).

The two **enterprise-grade weaknesses** are:

1. **Authorization model is binary owner-uid only.** There is no
   per-request capability tier. Every authorized peer can call
   `Shutdown`, `CryptoReset`, `AccountChangePassword`,
   `DeleteBackup`, etc. The "privileged request" list in
   `serve.rs::is_privileged_request` is **audit-only** — it logs but
   does not gate. A second authorized uid (e.g. an `Administrators`-
   style group, or a future multi-uid mode the docs already hint at)
   could shut the daemon down without further proof of intent.
2. **Web management surface has no authentication beyond same-uid
   IPC.** `pcloud-web` performs a same-process token + CSRF check,
   but the `web_token` is mode-0600 file-readable by any process
   running as the daemon owner — *which is the same threat boundary
   as IPC itself*. Documented honestly, but a same-uid attacker who
   reaches the loopback HTTP port or the token file gets full
   mutation rights. There is no rate limit on the web surface.

Net assessment: **production-deployable for a single-user, owner-only
posture**. Not yet ready for a multi-uid or operator/admin separation
model. CLAUDE.md should be corrected on the Windows IPC posture.

---

## Findings by severity

- CRITICAL: 0
- HIGH: 2
- MEDIUM: 5
- LOW: 4

---

## Detailed findings

### HIGH

#### H-7.1 — No per-request capability tier; "privileged" gate is audit-only
- **File:** `crates/pcloud-daemon/src/serve.rs:109-135` (`is_privileged_request`),
  `serve.rs:233-266` (`dispatch_with_drain_gate`),
  `crates/pcloud-ipc/src/server.rs:130-132` (`authorize_peer`).
- **Evidence.** `authorize_peer` returns `peer.matches_owner(self.owner_uid)` —
  a single boolean. `is_privileged_request` correctly enumerates
  shutdown, crypto reset, password change, auth persistence,
  sync-root remove, backup delete, etc., but the call site at
  `serve.rs:245-260` only emits `log::info!(...)`. The dispatch then
  proceeds for any authorized peer.
- **Risk.** If `owner_uid` is ever broadened (the comment at
  `serve.rs:252` already anticipates "future deployments where
  multiple authorized uids can share a socket"), every additional
  uid gets full destructive authority. Operators cannot deploy a
  read-only / monitoring uid without inventing a separate process.
- **Remediation.** Introduce a `Capability` enum (e.g.
  `ReadStatus`, `Mutate`, `Privileged`) and a per-`Method` /
  per-`Request`-variant requirement table. Resolve the peer to a
  `CapabilitySet` at accept-time (initially `{All}` for `owner_uid`,
  but the API can grow). Reject mismatched calls with
  `ResponseStatus::Unauthorized`. Tracker bead under `bd-1du.10`
  follow-up.

#### H-7.2 — Connection-cap state is **process-global** static, leaks across embedders
- **File:** `crates/pcloud-ipc/src/transport.rs:106-170` (statics
  `ACTIVE_CONNECTIONS`, `PEER_CONNECTIONS`, `MAX_IPC_CONNECTIONS_RUNTIME`,
  `MAX_IPC_CONNECTIONS_PER_PEER_RUNTIME`); `set_ipc_connection_caps`
  at `transport.rs:88-92`.
- **Evidence.** The connection-cap counters and runtime cap values
  are `static` and shared by every `BoundIpcServer` in the process.
  Tests under
  `crates/pcloud-ipc/src/transport.rs:1257-1336` already serialize
  on `PER_PEER_TEST_LOCK` to avoid leaking state. In production
  this is fine because there is only one daemon per process, but
  any future embedder that spins up two `BoundIpcServer` instances
  (e.g. one for management on a different socket path) will share
  caps and counters.
- **Risk.** A future multi-listener daemon (planned by the
  `pcloud-web` posture comments) would have one chatty surface
  starve the other. Not exploitable today.
- **Remediation.** Move the cap constants into a struct owned by
  `BoundIpcServer`, or accept a `ConnectionLimiter: Arc<Limiter>`
  at bind time so multiple embedders can opt in to independent
  caps. Backwards-compatible default: today's process-global
  behaviour.

### MEDIUM

#### M-7.3 — CLAUDE.md misrepresents Windows IPC posture (out of date)
- **Files:** `CLAUDE.md` (referenced in audit-prompt context) vs.
  `crates/pcloud-ipc/src/transport.rs:280-622` and
  `crates/pcloud-ipc/src/platform/windows.rs:1-989`.
- **Evidence.** CLAUDE.md states *"WindowsIpc compiles, but the
  named-pipe backend is not wired through serve_once_with_peer …
  pcloud_daemon::serve_with_shutdown returns Unsupported"*. The
  source contradicts this:
  - `transport.rs:439-477` includes a fully-wired `#[cfg(windows)]`
    arm in `BoundIpcServer::serve_once_with_peer` that calls
    `listener.accept()`, harvests `peer_pid`, runs the connection
    cap, and dispatches through `serve_stream_once_with_peer`.
  - `transport.rs:486-592` mirrors the same wiring in
    `accept_and_spawn`.
  - `serve.rs:533-604` (`serve_with_shutdown`) runs identically on
    Unix and Windows, including the scoped shutdown-watcher thread
    that drives `BoundIpcServer::request_shutdown` to wake a parked
    `ConnectNamedPipe` via the platform `CancelEvent` cancel path
    (`platform/windows.rs:325-541`).
  - `serve.rs:283` only documents that
    `set_accept_timeout` is a no-op on Windows; the cancel-event
    path is the documented replacement.
- **Risk.** Operators reading CLAUDE.md will under-deploy Windows.
  Tier classification is stale.
- **Remediation.** Update CLAUDE.md and `STATUS.md` to record that
  Windows IPC is wired end-to-end on the synchronous path; remaining
  Tier-1 gaps are write-timeout (`platform/windows.rs:670-678`,
  `set_write_timeout` is a documented no-op) and a live integration
  test (`platform_ipc_crossplat.rs:146-151` is `#[cfg(windows)]
  #[ignore]`).

#### M-7.4 — pcloud-web token confidentiality boundary equals the IPC boundary
- **File:** `crates/pcloud-web/src/lib.rs:299-337`
  (`write_web_token_to_runtime_dir`); `pcloud-web/src/routes.rs:719`
  (`require_web_token`).
- **Evidence.** The web management surface authenticates mutating
  routes via `X-PCloud-Web-Token` matched against a 64-hex token
  written to `${XDG_RUNTIME_DIR}/pcloud-daemon/web-token` with mode
  `0600`. The IPC socket lives in the same `0700` runtime dir and
  is also mode `0600`. **Any same-uid process can read the token
  file**, so the web surface confers no authentication advantage
  over IPC; it merely bridges HTTP to IPC. The `lib.rs:49-62`
  comment is explicit and honest about this.
- **Risk.** Operators may infer (incorrectly) that the web surface
  is "additional auth". It is not — it is a different transport
  with the same trust boundary.
- **Remediation.** Either (a) document this loudly in
  `OPERATIONS-RUNBOOK.md` so the deployment guide makes the
  equivalence explicit, or (b) raise the web surface's bar (e.g.
  HOTP-style per-session token, OS keyring storage) so it is
  meaningfully tighter than IPC. Today's posture is acceptable for
  single-user laptops, NOT for a shared-multi-user host.

#### M-7.5 — `pcloud-web` has no rate limit; CSRF + token are the only gates
- **File:** `crates/pcloud-web/src/routes.rs` (whole file: no
  rate-limit middleware, no per-IP / per-token bucket).
- **Evidence.** The router is built at
  `routes.rs:72-88` with `delete(...)`, `get(...).post(...)`. No
  `tower::limit` layer, no `tower_governor`, no token bucket. The
  daemon-side IPC rate limiter at
  `crates/pcloud-daemon/src/rate_limit.rs:90-141` does enforce a
  per-peer-uid bucket on the IPC handler — but every web request
  reaches the daemon as the same peer uid (the daemon owner), so
  the per-peer bucket effectively becomes a single bucket for *all*
  web traffic.
- **Risk.** A misbehaving local script that loops through the web
  surface can drive enough mutating traffic to e.g. exhaust public-
  link create quota, with the daemon's own rate limiter being the
  only backstop.
- **Remediation.** Add a `tower::limit::RateLimitLayer` on the
  mutating routes; consider keying off `X-PCloud-Web-Token` so each
  emitted token gets its own bucket.

#### M-7.6 — Privileged-request audit log lacks structured fields
- **File:** `crates/pcloud-daemon/src/serve.rs:254-260`.
- **Evidence.** The privileged-request hook emits a flat
  `log::info!("privileged IPC request: {} from uid={} pid={}", ...)`.
  No structured fields, no correlation id, no audit-chain row.
- **Risk.** Forensics after an incident (which uid invoked
  `CryptoReset`?) requires log scraping. The audit chain
  (`pcloud_store::append_audit_event`) is wired elsewhere but not
  here.
- **Remediation.** Append a `pcloud_store::append_audit_event(
  "ipc.privileged", {kind, uid, pid, ts})` row alongside the
  `log::info!` call. The audit-persistence-failure surface
  (`crates/pcloud-daemon/src/audit_verifier_service.rs`) already
  exists.

#### M-7.7 — Proptest schema enumerator wildcards over `#[non_exhaustive]`
- **File:** `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:120-123`
  (`must_match_every_method_variant`); `methods.rs:36-37, 261-262`
  (`#[non_exhaustive]`).
- **Evidence.** The exhaustiveness guard is a `match m { ... | ... |
  Method::VerifyEmail => 0, _ => 0 }`. Because the catch-all `_`
  arm collapses unknown variants to `0`, **adding a new `Method`
  variant will not fail compilation here** — the guard documents
  this in a maintenance comment but the contract is honor-system.
  `arb_request()` likewise enumerates by hand and will silently
  miss new variants.
- **Risk.** Coverage drift is not enforced by CI. A new IPC variant
  may slip in without proptest coverage.
- **Remediation.** Generate `every_method()` and `arb_request()` via
  a build-script-generated table or a `proc_macro_attribute` on
  `Method` / `Request` so the compiler enforces enumeration. Less
  invasive: replace the wildcard arm with `_ => unreachable!("new
  Method variant not yet enumerated in test")` so an addition lands
  as a runtime panic in CI.

### LOW

#### L-7.8 — Windows write-timeout is a no-op
- **File:** `crates/pcloud-ipc/src/platform/windows.rs:670-678`.
- **Evidence.** `set_write_timeout` returns `Ok(())` without
  installing a kernel-side deadline. The serve-loop already calls
  `set_write_timeout(IPC_RESPONSE_WRITE_TIMEOUT=30s)` on Unix
  (`transport.rs:172-173, 862, 906`). On Windows a malicious peer
  can stall a `WriteFile` indefinitely.
- **Risk.** Per-connection slow-client → response-write hang. Bound
  in practice by the connection cap (`MAX_IPC_CONNECTIONS=128`) so
  the daemon does not deadlock, but a single response can be
  pinned forever.
- **Remediation.** Use overlapped `WriteFile` + `WaitForSingleObject`
  with the configured timeout, or document the limitation in
  `bd-xplat-windows` and gate writability behind a watchdog.

#### L-7.9 — `current_effective_uid()` returns `0` on Windows
- **File:** `crates/pcloud-ipc/src/auth.rs:64-82`.
- **Evidence.** On Windows the helper returns `0` as a placeholder
  so the `PeerIdentity::matches_owner` gate keeps working
  (`uid=0` is also reported by the platform peer-uid path on
  successful SID match).
- **Risk.** A future caller that uses `current_effective_uid()` for
  audit / display will see `0` on Windows, conflating it with
  Unix-`root`. The bug is signposted but not yet exploited.
- **Remediation.** Either rename the function (e.g.
  `current_authority_handle()`) or make it `#[cfg(unix)]` and
  introduce a Windows-specific `current_user_sid_string()` shadow
  for the daemon side.

#### L-7.10 — Stress test `#[ignore]` by default; not in the routine CI gate
- **File:** `crates/pcloud-ipc/tests/stress_concurrent_clients.rs:64`.
- **Evidence.** `#[ignore = "stress: 50 clients x 500 reqs, run
  with --release --ignored"]`. fd-leak detection is also `cfg`-
  gated to Linux (`stress_concurrent_clients.rs:46-61`).
- **Risk.** Routine CI does not exercise the connection cap, the
  serve loop's high-concurrency invariants, or fd hygiene.
- **Remediation.** Add a nightly job that runs `cargo test -p
  pcloud-ipc -- --ignored stress_concurrent_ipc_clients`. Cross-
  platform fd-leak detection (macOS `sysctl KERN_PROC_FD`, BSD
  `procstat -f`) should be wired so the test is meaningful on
  Tier-1 platforms.

#### L-7.11 — `serve_loop_body` does not surface refresh-tick failures via metrics
- **File:** `crates/pcloud-daemon/src/serve.rs:609-643`.
- **Evidence.** The session-refresh tick logs `Err(e) =>
  log::error!(...)`. There is no counter increment for repeat
  errors; a stuck refresh would only appear as log spam.
- **Risk.** Silent operability degradation when the refresh path
  fails repeatedly.
- **Remediation.** Increment
  `pcloud_daemon_session_refresh_failures_total` (already wired in
  `pcloud-observability` for SLO `auth.refresh.error_rate`) on
  `Err(_)` and `TickOutcome::TemporaryFailure { .. }`.

---

## Per-platform IPC accept-loop wiring matrix

| Platform   | socket bind                                                                  | peer-cred check                                          | accept loop wired through `serve_once_with_peer` | version negotiation                                    |
|------------|------------------------------------------------------------------------------|----------------------------------------------------------|--------------------------------------------------|--------------------------------------------------------|
| Linux      | `transport.rs:730-737` (UnixListener, `0600`, `0700` parent)                 | `SO_PEERCRED` (`platform/linux.rs:31-89`)                | YES (`transport.rs:397-438`)                     | YES (`protocol.rs:268-308`, `MIN_ACCEPTED..=CURRENT`)  |
| FreeBSD    | same as Linux                                                                | `getpeereid(3)` (`platform/unix.rs:29-67`)               | YES (same Unix arm)                              | YES                                                    |
| OpenBSD    | same                                                                         | `getpeereid(3)`                                          | YES                                              | YES                                                    |
| NetBSD     | same                                                                         | `getpeereid(3)`                                          | YES                                              | YES                                                    |
| macOS      | `transport.rs:730-737` + launchd activation (`transport.rs:638-696`)         | `getpeereid(3)`                                          | YES                                              | YES                                                    |
| Windows    | `\\.\pipe\pcloud-rs-<hex-SID>` + DACL (`platform/windows.rs:286-298,758-797`)| `GetNamedPipeClientProcessId` + `TokenUser` SID compare (`platform/windows.rs:484-511,819-872`) | **YES** (`transport.rs:439-477`, `486-592`)      | YES (same protocol decoder)                            |

**Correction to CLAUDE.md.** The Windows accept loop is wired and
`serve_with_shutdown` runs the same body on Windows. CLAUDE.md's
"returns Unsupported" line is stale.

---

## Per-Request capability requirement table

There is **no formal capability tier today**. Every authorized peer
(`peer.uid == owner_uid`) can call every variant. The table below is
**advisory** — what *should* be enforced. Source of the
"privileged" classification is `serve.rs::is_privileged_request`
(audit-only as of today). Unmarked Requests are read-only.

| Capability tier         | Requests                                                                                                                                                                                                                                                                                                                                                                                       | Today's enforcement                  |
|-------------------------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|--------------------------------------|
| **PRIVILEGED (elevated)** | `Plain { Shutdown, CryptoReset, SetAuthPersistence, SendCryptoChangeUserPrivate }`, `AccountChangePassword`, `CryptoSetup`, `CryptoSetupV2`, `CryptoChangePassword`, `CryptoChangePasswordUnlocked`, `CryptoGetFolderKey`, `CryptoGetFileKey`, `AuthPersistence`, `SyncRootRemove`, `DeleteBackup`, `CreateBackup`, `StopDevice`, `DeleteBackupDevice`, `LostPassword`, `VerifyEmailRestricted`, `UploadWriteFromFile`, `CreateTreePublicLinkFromPaths` | owner-uid only + `log::info!` audit  |
| **Mutate**              | `SyncRootAdd`, `SyncRootPause`, `SyncRootResume`, `SyncRootChangeType`, `Mount`, `Unmount`, `MountForceUnmount`, `CreateRemoteFolder`, `CreateFolderByPath`, `RenamePath`, `WriteFileFresh`, `FileDeleteByPath`, `FolderDeleteByPath`, `FolderDeleteById`, `UploadCreate`, `UploadPause`, `UploadResume`, `UploadCancel`, `IntegrityRunOnce`, `IntegritySkip`, `BackupSnapshot`, `ConflictResolve`, `Create*PublicLink`, `Change*PublicLink`, `Delete*PublicLink`, `Add/RemovePublicLinkAccess`, `ListBookmarks`/`ChangeBookmark`/`RemoveBookmark`, `ShareFolder`, `Cancel/Decline/AcceptShareRequest`, `RemoveShare`, `ModifyShare`, `AccountStopShare`, `AccountModifyShare`, `AccountTeamShare`, `ValueSet`, `MarkNotificationsRead`, `SendPublink`, `SetApiServer`, `SetLanguage`, `SubmitPassword`, `SubmitTwoFactorCode`, `LoginBegin`, `Logout`, `UnlockCrypto`, `LockCrypto`, `CryptoMkdir`, `RunLocalScan`, `PauseSync`, `ResumeSync`, `AccountRegister`, `VerifyEmail`, `DownloadFile`, `PasswordSubmission`, `AuthTokenSubmission`, `TwoFactorCodeSubmission` | owner-uid only                       |
| **Read**                | `Plain { GetStatus, GetHealth, Health, GetPending, GetSyncRoots, ListPublicLinks, ListUploadLinks, GetUserInfo, ListIncoming/OutgoingShares, ListIncoming/OutgoingShareRequests, ListContacts, ListMyTeams, ListNotifications, GetCryptoStatus, GetCryptoPrivKeyFlags, GetCryptoHint, SessionStatus, FileHistory, IntegrityStatus, HaStatus, DrainStatus, GetSlo, GetAuditVerifierStatus, GetSyncStatus, ListConflicts, StatPath, GetApiServers, GetPromo }`, `ShowPublicLink`, `IsFolderSyncable`, `GetSyncSuggestions`, `ConflictList`, `UploadList`, `StatPath`, `ListFolderByPath`, `GetFolderIdByPath`, `GetFolderFlags`, `GetFolderOwnerId`, `FilesystemStatus`, `VerifyPath`, `SessionStatus`, `ValueGet`, `ValueHas`, `AuditVerifyChain`, `GetFileLink`, `ListPublicLinkAccess` | owner-uid only                       |

**Current effective gate:** all three tiers collapse to "is the peer
uid equal to the daemon owner uid?" (`server.rs:130-132`).

---

## Cross-cut observations

- **Wire format.** 8-byte LE header (`u32 payload_len | u16 version
  | u16 message_type`) + JSON body, capped at `MAX_IPC_PAYLOAD_LEN
  = 1 MiB`. Decoder rejects oversize *before* allocation
  (`transport.rs:911-929`). `MIN_ACCEPTED_IPC_PROTOCOL_VERSION = 1`
  with one-version rolling-upgrade window documented at
  `protocol.rs:41-55`.
- **Serde proptest coverage.** ~70 `Request` variants
  (`proptest_methods_roundtrip.rs:191-677`) plus a `prop_random_bytes_do_not_panic`
  fuzz-shaped harness (`:715-718`). New variants since the
  manually-listed `every_method()` (e.g. `Health`, `FileHistory`,
  `IntegrityStatus`, `HaStatus`, `DrainStatus`, `GetSlo`,
  `GetAuditVerifierStatus`, `GetSyncStatus`, `ListConflicts`,
  `StatPath`, `GetApiServers`, `GetPromo`, `GetCryptoHint`,
  `VerifyEmail`) appear in the exhaustive `match` guard but **not**
  in the `every_method()` runtime list. See M-7.7.
- **Runtime dir hygiene.** `bootstrap.rs:441-453` chmods
  `paths.runtime_dir` to `socket_dir_mode` (default `0o700`),
  state/cache/config dirs to their respective `*_mode` fields.
  Socket lives at `runtime_dir/pcloud.sock`
  (`pcloud-config/src/paths.rs:92-93`). On `Drop`,
  `BoundIpcServer` unlinks the socket
  (`transport.rs:625-629`). On macOS, stale `*.sock` files are
  swept on startup (`main.rs:111-121`).
- **Graceful shutdown.** Three-state machine in
  `signals::DrainState`. `serve.rs:355-498` (`serve_loop_body`)
  observes the shutdown flag, transitions Running→Draining,
  quiesces the mount, polls `in_flight == 0` against a
  `drain_timeout_secs` deadline, and exits clean. Drain-aware
  dispatch admits `DrainStatus`, `Shutdown`, `GetHealth`, `Health`
  (`serve.rs:212-220`). Tested by
  `crates/pcloud-daemon/tests/graceful_drain.rs`.
- **Crash recovery.** `bootstrap.rs:767-827` runs orphan-mount
  detection on startup via `MountControl::check_orphans` and
  `force_unmount`, sweeping stale mount-pid files first. Upload
  journal replay tested by `tests/upload_journal_crash_replay.rs`.
- **Stress.** `tests/stress_concurrent_clients.rs:31-32`
  exercises 50 clients × 500 requests. `#[ignore]` by default; see
  L-7.10.
- **Web/management surface.**
  `pcloud-web/src/lib.rs:373-378` enforces loopback bind via
  `assert!`. `routes.rs:78-88` builds the router; mutating routes
  go through `require_web_token` + `require_csrf`. No TLS, no
  per-IP rate limit (M-7.5). Socket-equivalent threat boundary
  (M-7.4).

---

## Honesty check vs. CLAUDE.md & STATUS.md

- CLAUDE.md "Windows IPC accept loop NOT wired" — **inaccurate.** See M-7.3.
- CLAUDE.md "`serve_with_shutdown` returns `Unsupported` on Windows"
  — **inaccurate.** Source returns the same `serve_until_shutdown_with_flag`
  result on every OS (`serve.rs:533-604`).
- CLAUDE.md "pcloudd-svc compiles + starts but is no-op stub" —
  **partially accurate.** The Windows Service binary itself is not
  in `crates/pcloud-daemon/src/`, but the underlying serve path it
  would consume (`serve_with_shutdown`) is real.
- CLAUDE.md security rules ("owner-only IPC, malformed/slow client
  isolation, audit persistence failures surfaced") — confirmed in
  source. Slow-client cleanup at
  `transport.rs:931-963`; framing cap test at
  `tests/request_size_cap.rs:78-184`.
- CLAUDE.md "BSD/Windows mount cleanup is Tier-3" — out of scope
  for this dimension; do not flip.

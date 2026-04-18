# Section 7: IPC & Daemon
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 7)

## Findings

### CRITICAL [0]

### HIGH [3]
1. `proptest_methods_roundtrip.rs` coverage gap — strategies exercise only ~21 of ~81 `Request` variants.
2. No active-connection cap / concurrent-client admission gate (`MAX_IPC_CONNECTIONS` absent).
3. `Request::VerifyPath` is defined in `methods.rs` and wired in the CLI but has **no handler** in `runtime.rs` — falls through to the "newer client than daemon?" catch-all.

### MEDIUM [6]
4. `Method::Shutdown` / `Request::Unmount` / `Request::MountForceUnmount` have no elevated-capability check beyond owner-UID.
5. Daemon `bootstrap_with_config` does NOT unlink the IPC socket on early-bootstrap panic paths.
6. Runtime directory ownership not explicitly verified at bind time (only mode set).
7. `ResponseStatus::PolicyViolation` variant absent from `dispatch::status_str()` and proptest strategies.
8. No re-adoption of orphaned FUSE mounts beyond warning log; no journal-based upload auto-resume.
9. `pcloud-web` routes (`sync_add`, `sync_remove`, `publinks_create`, `publinks_revoke`) accessible to any local process that can reach `127.0.0.1:17650`, with no auth token gate.

### LOW [5]
10. Single-threaded serve loop — a slow client stalls all others for up to 5 s read-timeout.
11. `write_pid_file` (main.rs:202) best-effort; no fsync of parent dir after rename.
12. `PeerIdentity.pid` reported as `0` on BSD/macOS (`transport.rs:396-398`).
13. `transport.rs:247-252` creates runtime-dir with 0700 only when `parent_missing`; pre-existing wider mode is NOT tightened.
14. `dispatch::backend_label` has `_ => "other"` fall-through (line 207) — new `Request` variants silently label as "other" in metrics.

---

## Detailed Findings

### 1. Wire format

- Length-prefix framing: correct. 8-byte header `u32 payload_len | u16 version | u16 message_type` at `crates/pcloud-ipc/src/protocol.rs:148-157`.
- `IPC_PROTOCOL_VERSION = 1` (`protocol.rs:39`), `MAX_IPC_PAYLOAD_LEN = 1 MiB` (`protocol.rs:47`), `MAX_REQUEST_BYTES = 1 MiB` (`server.rs:42`). Both caps enforced **before** `Vec::with_capacity` allocation at `transport.rs:308-317`.
- Version negotiation: strict — any mismatch returns `ProtocolError::VersionMismatch` at `protocol.rs:255-260`.
- Forward/backward compat: `Method` and `Request` are `#[non_exhaustive]` (`methods.rs:36, 261`). Unknown `Request` variants hit runtime's catch-all at `runtime.rs:810-815` → `InvalidRequest`. Correct.

### 2. Serialization safety — HIGH: proptest coverage gap

`crates/pcloud-ipc/src/methods.rs`: **~45 `Method` variants** and **~81 `Request` variants** counted.

`crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs::arb_request` (lines 145-200) exercises only **~21 variants**. Missing from proptest strategies include:

`DeletePublicLinkByCode`, `ChangePublicLinkUpload`, `CreateUploadLink`, `DeleteUploadLink`, `CreateTreePublicLink`, `ListPublicLinkAccess`, `AddPublicLinkAccess`, `RemovePublicLinkAccess`, `ChangeBookmark`, `ShareFolder`, `CancelShareRequest`, `DeclineShareRequest`, `AcceptShareRequest`, `RemoveShare`, `ModifyShare`, `AccountStopShare`, `AccountModifyShare`, `AccountTeamShare`, `MarkNotificationsRead`, `AuditVerifyChain`, `Mount`, `CreateRemoteFolder`, `Unmount`, `MountForceUnmount`, `RunLocalScan`, `SendPublink`, `GetFolderIdByPath`, `GetFolderFlags`, `GetFolderOwnerId`, `FilesystemStatus`, `FileHistory`, `VerifyPath`, `BackupSnapshot`, `IntegrityRunOnce`, `IntegritySkip`, `UploadCreate`, `UploadPause`, `UploadResume`, `UploadCancel`, `UploadList`, `ConflictList`, `ConflictResolve`, `StatPath`, `LostPassword`, `VerifyEmailRestricted`, `AccountChangePassword`, `AccountRegister`, `GetFileLink`, `DownloadFile`, `DeleteBackup`, `SetApiServer`, `SetLanguage`, `SessionStatus`, `SyncRootChangeType`, `GetSyncSuggestions`, `CryptoMkdir`, `CryptoChangePassword`, `CryptoChangePasswordUnlocked`.

Similarly, `every_method()` (lines 15-48) lists 28 of 45 `Method` variants, missing: `Health`, `FileHistory`, `IntegrityStatus`, `HaStatus`, `DrainStatus`, `GetSlo`, `GetAuditVerifierStatus`, `GetSyncStatus`, `ListConflicts`, `StatPath`, `GetApiServers`, `GetPromo`, `GetCryptoHint`, `VerifyEmail`, `SessionStatus`.

**Fix:** Extend `arb_request` and `every_method()` to cover every variant. Add a compile-time exhaustiveness assertion (similar to `must_match_every_method_variant`) that fails when a new variant is added without a strategy.

### 3. Auth & authorization

- Peer-cred verification: `crates/pcloud-ipc/src/transport.rs::peer_identity` (line 386-399) — Linux calls `SO_PEERCRED` via `platform/linux.rs:94-120`, BSD/macOS call equivalent. Verified on every accept at `transport.rs:186-198`. ✓
- Owner-UID check at `transport.rs:199-208` rejects non-owner with `Unauthorized`. ✓
- **Per-request capability checks — weak (MEDIUM):** `Method::Shutdown` dispatches to `runtime.rs:450` without any capability gate beyond "same UID". `Request::Unmount`, `Request::MountForceUnmount`, and destructive `BackupSnapshot` actions similarly rely solely on UID match. **Fix:** introduce a privileged-method allow-list with an in-band confirmation mechanism.
- `Request::TwoFactorCodeSubmission.value`: **confirmed fixed** — `methods.rs:292` now uses `RedactedString`. Debug output is redacted. ✓

### 4. Runtime directory hygiene — MEDIUM

`crates/pcloud-ipc/src/transport.rs:247-267`:
- Parent directory created with `0o700` **only when newly created** (line 250). Pre-existing wider-mode parent is NOT tightened. An attacker who pre-creates the runtime dir with 0755 gets persistent access.
- Socket mode `0600`: line 260 — correct. ✓
- No `chown` verification of parent directory ownership.
- `BoundIpcServer::Drop` (`transport.rs:232-236`) unlinks the socket on normal exit. ✓
- Panics during bootstrap before `BoundIpcServer` is constructed leave no socket stale but may leave a pidfile behind. No `catch_unwind` in `main.rs`.

**Fix:** Always call `set_permissions(0o700)` on the runtime dir regardless of whether it pre-exists; verify `metadata.uid() == current_uid()`.

### 5. Graceful shutdown — GOOD

`crates/pcloud-daemon/tests/graceful_drain.rs` **exists**. Verifies: pre-drain state, external flag flip, drain-gate rejects ordinary traffic, status answers during drain, clean loop exit, socket unlinked.

Drain state machine (`signals.rs`): three-state Running/Draining/Stopped. `InFlightGuard` RAII increments/decrements. `serve.rs:127-231` loop: on shutdown → `begin_drain()`, polls `in_flight() == 0` or timeout. `quiesce_for_drain()` called on mount at line 156.

**Gap (MEDIUM):** `Request::UploadCancel` and `Request::UploadPause` cannot be invoked during drain — uploads stuck until timeout. Consider exempting these from the drain rejection.

### 6. Crash recovery — MEDIUM gaps

- `bootstrap.rs:548-587`: upload-sidecar enumeration — **log-only; no auto-resume**. Operator must act.
- `bootstrap.rs:742-802`: orphan FUSE mount scan via `MountControl::check_orphans`. On orphans detected: log error, force-unmount only if `PCLOUD_FORCE_UMOUNT=1`, else refuse to start. Deliberate — no silent re-adoption.
- Sync state re-hydration: handled by `sync_loop_runtime::spawn_daemon_sync_loop` on each start. Tier-2 HA lease at `bootstrap.rs:648-695` prevents split-brain.

### 7. HIGH: No connection cap

`crates/pcloud-ipc/src/transport.rs` — no `MAX_IPC_CONNECTIONS` or per-source semaphore anywhere. `serve_once` is sequential (one connection at a time). A slow client holding the 5 s read-timeout blocks all other clients.

`crates/pcloud-ipc/tests/stress_concurrent_clients.rs` **exists** (`#[ignore]`-gated, 50 clients × 500 requests, asserts no fd leak, all requests served, socket cleaned up at lines 135-155). But the test does not exercise the starvation path.

**Fix:** Enforce a bounded semaphore (e.g., `tokio::sync::Semaphore`) in `serve_until_shutdown_with_flag` limiting concurrent accepted connections.

### 8. HIGH: Request::VerifyPath — orphaned handler

`crates/pcloud-ipc/src/methods.rs:807-816`: `Request::VerifyPath { path, recursive }` is declared with a doc-comment describing per-file SHA256 streaming output.

`crates/pcloud-daemon/src/runtime.rs`: **zero hits** for `VerifyPath` in `handle_request_dispatch`. Falls through to the `_ =>` catch-all at `runtime.rs:810-815`, returning `InvalidRequest` ("unsupported ipc request (newer client than daemon?)") — **misleading** because both sides are the same crate version.

`crates/pcloud-cli/src/commands.rs:1158` and `crates/pcloud-cli/src/app.rs:3795` actively construct this variant and dispatch it. Every invocation silently fails.

`crates/pcloud-daemon/src/dispatch.rs` also has no arm for `VerifyPath` → buckets to "other" in traces.

**Fix:** Add `Request::VerifyPath { path, recursive } => self.verify_path(path, recursive)` with a handler that walks the sync-root tree and streams per-file integrity results. Add the arm to `dispatch::backend_label`. Add an integration test.

### 9. Slow/malformed client isolation

- Per-connection read timeout: `IPC_REQUEST_READ_TIMEOUT = Duration::from_secs(5)` (`transport.rs:32`), applied at `transport.rs:184`. Client never-sends → times out after 5 s. ✓
- Byte budget: one framed request, capped at 1 MiB, enforced pre-allocation. ✓
- Oversized-frame policy: close without replying (`transport.rs:337-340`). ✓
- **LOW gap:** Write timeout NOT set on the response stream. A client that connects, sends a valid request, then never reads the response stalls the serve thread on `stream.write_all` / `stream.flush` (`transport.rs:373-374`). **Fix:** add `set_write_timeout(Some(IPC_REQUEST_READ_TIMEOUT))` after reading the request.

### 10. Web / management surface (pcloud-web) — MEDIUM

`crates/pcloud-web/src/lib.rs`:
- Bind address: loopback-only (`127.0.0.1:17650`), enforced via panic at `lib.rs:237-242,282-286`. ✓
- **Auth: NONE.** Any local process reaching `127.0.0.1:17650` can invoke `sync_add`, `sync_remove`, `publinks_create`, `publinks_revoke` without any token. In a multi-user environment this is a privilege escalation path (the daemon socket is 0600 per-owner, but the web port is open to all local users).
- TLS: none — HTTP only on loopback. Acceptable for same-host model.
- CSP header: `default-src 'self'; script-src 'none'; style-src 'self' 'unsafe-inline'` — restrictive. ✓

**Fix:** Add a per-session token (set via daemon IPC, verified in web handler) or bind to an abstract Unix socket instead of a TCP port.

### 11. Dispatch coverage

`runtime.rs::handle_request_dispatch` (lines 353-817) handles every documented variant **except `Request::VerifyPath`** (see §8). The `_ =>` fall-throughs are required by `#[non_exhaustive]` and are correct, but the catch-all conflates "unknown future variant" with "known-but-unhandled variant". See §8 for fix.

### 12. Integration test coverage

`crates/pcloud-daemon/src/lib.rs` integration tests exercise approximately **30 of ~81 Request variants** (≈ 37%). Missing coverage includes: `VerifyPath`, `Mount`, `Unmount`, `MountForceUnmount`, `BackupSnapshot`, `IntegrityRunOnce`, `IntegritySkip`, `UploadCreate/Pause/Resume/Cancel/List`, `AuditVerifyChain`, `CryptoChangePassword{Unlocked}`, `CryptoMkdir`, `StatPath`, `FileHistory`, `FilesystemStatus`, `GetFolderIdByPath`, `GetFolderFlags`, `GetFolderOwnerId`, `ConflictList`, `ConflictResolve`, `LostPassword`, `VerifyEmailRestricted`, `AccountChangePassword`, `AccountRegister`, `GetFileLink`, `DownloadFile`, `DeleteBackup`, `SetApiServer`, `SetLanguage`, bulk share variants.

---

## Remediation Priority

| Priority | Finding | Fix |
|----------|---------|-----|
| HIGH-1 | `Request::VerifyPath` has no handler | Add handler in `runtime.rs`; add dispatch label arm; add integration test |
| HIGH-2 | proptest covers ~21/81 variants | Extend `arb_request`; add exhaustiveness compile test |
| HIGH-3 | No connection cap | Add bounded semaphore in serve loop |
| MEDIUM-4 | Runtime dir perm not tightened | Always `set_permissions(0o700)` + verify uid |
| MEDIUM-5 | No capability gate on destructive ops | Add privileged-method allow-list |
| MEDIUM-6 | pcloud-web no auth | Add per-session token or move to Unix socket |
| MEDIUM-7 | `PolicyViolation` missing from `status_str` | Add arm; add proptest strategy |
| LOW-8 | Write timeout missing | Add `set_write_timeout` after request read |
| LOW-9 | Pidfile no parent fsync | Add `sync_parent_directory` call |

# Stream G6a — IPC Wire & Daemon Lifecycle Fixes

**Audit source:** `GPTREV/06_ipc_daemon_web_config.md` (IPC + daemon sections only)
**Date:** 2026-04-26

## Scope

IPC wire format / framing / version negotiation, daemon lifecycle / runtime / dispatch,
authorization / capability checks, crash recovery / state re-hydration, tests.

Findings addressed: MEDIUM-08, MEDIUM-09, MEDIUM-13, LOW-14, LOW-15, HIGH-01.
Findings deferred (owned by other streams): HIGH-02 (capability layer), HIGH-03..07 (web/windows/socket), MEDIUM-10 (transit String rationale already documented in audit H1), MEDIUM-11, MEDIUM-12.

---

## Fixes Applied

### MEDIUM-08 — IPC decoders accept frames tagged as wrong message kind

**File:** `crates/pcloud-ipc/src/protocol.rs`

Added `ProtocolError::WrongMessageKind { expected, actual }` variant. Updated
`decode_request` to reject frames whose `message_type != MessageKind::Request`
**before** deserializing the payload, and `decode_response` symmetrically for
`MessageKind::Response`. The check fires after the size cap but before any
`serde_json::from_slice` call so adversarially-crafted payloads in the wrong
frame type never reach the JSON decoder.

Two new unit tests added in `protocol.rs::tests`:
- `decode_request_rejects_response_tagged_frames`
- `decode_response_rejects_request_tagged_frames`

### MEDIUM-09 — Windows client allocates response buffer before enforcing payload cap

**File:** `crates/pcloud-ipc/src/transport.rs`

In the `#[cfg(windows)]` branch of `IpcClient::send_envelope`, added a
`MAX_IPC_PAYLOAD_LEN` check (now imported from `protocol`) immediately after
reading the 8-byte frame header and before `vec![0u8; payload_len]`. A
compromised/spoofed daemon endpoint can no longer force unbounded allocation on
Windows clients.

### MEDIUM-13 — Mount crash-recovery refusal is logged but not enforced

**Files:** `crates/pcloud-daemon/src/mount_runtime.rs`, `crates/pcloud-daemon/src/bootstrap.rs`

Added `disabled_reason: Option<String>` field to `MountControl`. Added
`MountControl::disable(reason)` and `MountControl::disabled_reason()` methods.
Updated `MountControl::mount()` to return `ResponseStatus::Conflict` immediately
when `disabled_reason.is_some()`, before any other validation. Wired the call in
`bootstrap.rs` at the `OrphanCheckOutcome::Rejected` arm so the controller is
actually disabled when bootstrap logs the refusal.

### LOW-14 — Graceful mount shutdown leaves stale `mount_pid` sidecar on Drop path

**File:** `crates/pcloud-daemon/src/mount_runtime.rs`

Added `self.remove_mount_pidfile()` call at the end of `MountControl::drop()`,
after `ordered_shutdown` completes. The explicit `unmount()` path already removes
the sidecar; this fix covers the SIGTERM/Drop path. A successful daemon teardown
no longer looks like a crash on the next boot.

### LOW-15 — TFA/recovery invalid code leaves session state as `AuthenticatingWithPassword`

**File:** `crates/pcloud-auth/src/manager.rs`

Added `self.snapshot.state = SessionState::TwoFactorRequired;` to the
`AuthCommand::MarkTwoFactorCodeInvalid` arm in `SessionManager::apply`. Previously
`SubmitTwoFactorCode` transitions to `AuthenticatingWithPassword`; a rejected code
returned `LoginFailed` without restoring the promptable `TwoFactorRequired` state,
violating the documented transition table. The `pending_challenge` is still
preserved for retry.

Regression test added: `mark_tfa_code_invalid_restores_two_factor_required`
exercises the full submit → reject → state-check → challenge-preserved → error-recorded cycle.

### HIGH-01 — `SetApiServer` persists rejected API host hints and reports success

**File:** `crates/pcloud-daemon/src/runtime.rs`

Added `SetApiServerError { Rejected(&'static str), Store(StoreError) }` type.
Changed `RuntimeShell::set_api_server` return type from `Result<(), StoreError>`
to `Result<(), SetApiServerError>`. The function now returns
`Err(SetApiServerError::Rejected)` **before** touching any live transport or
persisted preference when `apply_api_server_hint` rejects the hint.

Updated `set_api_server_ipc` to map `Rejected` to `ResponseStatus::InvalidRequest`
(not `InternalError`). Previously a rejected hint would silently apply to all
live runtimes and persist to preferences, then return `Ok`.

---

## Test Results

```
cargo check -p pcloud-ipc -p pcloud-daemon
  → Finished `dev` profile (0 errors, 0 warnings)

cargo test -p pcloud-ipc -p pcloud-daemon --lib
  → 213 pcloud-daemon lib tests: ok
  → 19 pcloud-ipc lib tests: ok

cargo test -p pcloud-ipc --test proptest_methods_roundtrip
  → 5 proptest tests: ok (prop_request_round_trips, prop_response_round_trips,
    prop_every_method_plain_round_trip, every_method_variant_round_trips,
    prop_random_bytes_do_not_panic)

cargo test -p pcloud-ipc
  → 24 total (lib + integration + doctests): ok

cargo test -p pcloud-auth --lib
  → 26 tests: ok (including new mark_tfa_code_invalid_restores_two_factor_required)
```

No regressions. The pre-existing flaky `read_upload_payload_zero_copy_for_large_files`
test (resource contention under parallel test execution) was confirmed pre-existing
by stashing changes and running it in isolation — it passes in isolation, both before
and after these changes.

---

## Files Modified

- `crates/pcloud-ipc/src/protocol.rs` — WrongMessageKind error + decode enforcement + tests
- `crates/pcloud-ipc/src/transport.rs` — Windows client MAX_IPC_PAYLOAD_LEN cap
- `crates/pcloud-auth/src/manager.rs` — MarkTwoFactorCodeInvalid state fix + regression test
- `crates/pcloud-daemon/src/mount_runtime.rs` — disabled_reason field + Drop pidfile cleanup
- `crates/pcloud-daemon/src/bootstrap.rs` — wire disable() on orphan rejection
- `crates/pcloud-daemon/src/runtime.rs` — SetApiServerError type + set_api_server early-reject

---

## Out-of-Scope (not addressed)

- **HIGH-02** (capability layer beyond same-UID) — requires a new token/grant
  subsystem touching many call sites; not in stream G6a scope.
- **HIGH-03..HIGH-07** (web token, socket binding, Windows service, CSRF) — owned
  by stream G6b (web/config).
- **MEDIUM-10** (TFA code as plain String in Request) — the audit H1 note in
  `methods.rs` already documents the rationale (transit-only, ephemeral lifetime,
  owner-only socket). A structural change requires serde-skip on `SecretString`
  which is a broader API change tracked separately.
- **HIGH-06** (Windows overlapped I/O) — requires platform-specific async pipe
  work tracked under `bd-xplat-windows`.

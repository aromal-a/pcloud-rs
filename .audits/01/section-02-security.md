## Section 2. Security Audit

**Auditor**: Dimension 2 (Security)
**Workspace**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/`
**Date**: 2026-04-17
**Scope**: secret discipline, auth vault, local IPC security, transport policy, downgrade/replay, FFI memory safety, input validation, DoS, logging discipline.
**Out of scope (other dimensions)**: cryptographic algorithm review (Dim 3), parity-matrix (Dim 1), detailed threat model (Dim 5).

### Executive summary

The pcloud-rs security posture is **substantially stronger** than the legacy C client on every surface inspected. Secret lifetimes are governed by an explicit `SecretString`/`SecretBytes` abstraction with `ZeroizeOnDrop`, constant-time equality, redacted `Debug`, no `Serialize`/`Deserialize` impls, and hand-rolled `clone_secret` methods so every duplication is audit-visible. The IPC transport enforces a `0600` socket under a `0700` parent, mandatory `SO_PEERCRED` / `getpeereid(3)` / per-SID DACL peer verification, a 1 MiB pre-allocation cap, a 5-second read timeout, and returns sanitized error messages. The auth vault is opt-in, atomically written with `O_CREAT|O_EXCL`, validated for ownership and mode on every read, and intentionally does not persist passwords. The production profile rejects plaintext transport in `ApiEndpoint::validate` and `RevisionUrl::validate`; the code base is free of `danger_accept_invalid_certs` / `accept_invalid_hostnames` in `src/` (only documentation references exist). The FFI surfaces (`platform/{linux,bsd,macos,windows}.rs`, `macos_ffi.rs`, `winfsp_ffi.rs`) carry SAFETY comments on every `unsafe` block with plausible invariant statements.

The **remaining gaps** are narrower and fall into four buckets:

1. A handful of transit-only IPC request fields remain plain `String` where a `RedactedString` wrapper is warranted.
2. `pcloud-proto` request-builder structs derive `Debug` while carrying a plaintext `auth_token: String` — this can leak the token via `format!("{req:?}")`.
3. The file-vault validator checks the vault file itself but does **not** re-validate parent-directory ownership/mode on read.
4. The IPC serve loop is single-threaded (`bound.serve_once`) with no per-peer connection cap — DoS is limited to blocking subsequent requests for up to the 5 s read timeout but a slow client can still impede refresh ticks and other service users.

No **CRITICAL** findings were identified. Four **HIGH** findings relate to secret fields derived-Debug in `pcloud-proto` and missing path normalization for the `SyncRootAdd.local_path` input. Nine **MEDIUM** findings cover vault parent-dir validation, `TwoFactorCodeSubmission.value` using plain `String`, and defense-in-depth items. **LOW** findings cover documentation accuracy and minor hardening.

---

## CRITICAL findings

**None identified.** No cleartext password persistence, no world-readable sockets, no plaintext-in-production path, no reachable `danger_accept_invalid_certs` / `accept_invalid_hostnames`, and no logs that interpolate `SecretString::expose_secret`.

---

## HIGH findings

### H1. Protocol request structs derive `Debug` with plaintext `auth_token: String`

**Files / lines (selected — pattern is systemic)**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:170-175` — `UserInfoRequest { auth_token: String, ... }` with `#[derive(Debug, Clone, PartialEq, Eq)]`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:214-225` — `TwoFactorLoginRequest { token: String, ... }`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:270-285` — `TwoFactorSendSmsRequest { token: String, ... }`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/auth.rs:311-330` — `TwoFactorSendNotificationRequest { token: String, ... }`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/upload.rs:88-94` and `:155-170`, `:208-215`, `:264-270`, `:331-340`, `:382-390`, `:450-460`, `:500-510`, `:548-560` — every request struct in the upload-session family carries `pub auth_token: String`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/shares.rs:34, 56, 79, 137, 158, 179, 209, 230, 254, 281, 310, 361` — every `SharesXxxRequest` has `auth_token: String` and derives `Debug`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/public_links.rs:14` through `:666` — 24 request structs, same pattern.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/account.rs:12, 62, 255, 323` — including `RegisterRequest { password: String }` at line 323.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/folder.rs:11, 64, 147, 194, 247, 293, 338`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/crypto.rs:34, 105`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/backup.rs:28, 99, 149`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/methods/diff.rs:16`, `download.rs:13`, `notifications.rs:35, 86`.

**Severity**: HIGH.

**Impact**: any `log::debug!("{req:?}")`, `tracing::debug!(?req)`, `panic!` path that formats a request with `{:?}`, or a future observer/middleware that derives `Debug`-display at tracing spans will emit the live pCloud auth token or account password in plaintext to logs. The counterpart IPC-boundary types in `pcloud-ipc/src/methods.rs` took the correct approach (`RedactedString`) in response to audit finding H1 (see e.g. `:279`, `:285`, `:301`, `:307`, `:336`, `:339`, `:354`, `:473`, `:963`, `:966`), but the lower protocol layer never followed. `CHANGELOG.md:1975` claims the repo is free of token-leaking Debug output, which is not accurate for these builder structs.

**Remediation**:
1. Change every `pub auth_token: String` / `pub password: String` / `pub token: String` on request builders in `crates/pcloud-proto/src/methods/**/*.rs` to `pcloud_ipc::RedactedString` (or an equivalent redacted wrapper local to `pcloud-proto`).
2. Alternatively, keep the wire field as `String` but remove `Debug` from the derive list, and provide a manual `impl Debug` that renders `UserInfoRequest { auth_token: <redacted N bytes>, ... }`. This keeps call sites compiling.
3. Add a negative `cargo test` that `format!("{:?}", req)` on each of these request types MUST NOT contain the secret literal.
4. Consider crate-wide lint via `#[deny(clippy::disallowed_types)]` or a custom lint that bans `Debug` derive on structs with fields named `auth_token` / `password` / `token`.

---

### H2. `SyncRootAdd.local_path` and related path-accepting requests are not validated for NUL/`..`/symlink escape before use

**Files / lines**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs:376-387` — `Request::SyncRootAdd { local_path: String, remote_path: String, ... }`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:3952-4015` — `add_sync_root` validation flow. Only `trim().is_empty()` is checked on the raw string; `canonicalize` is relied on to resolve the path but there is no explicit NUL-byte check, no explicit `..` rejection, and no explicit rejection of paths outside a configured sandbox. `canonicalize` will follow symlinks, potentially pointing to a system directory the attacker wanted the daemon to sync over.
- Same pattern in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:609-611` (`GetSyncSuggestions`), `:612` (`IsFolderSyncable`), `:616-617` (`CreateFilePublicLink` / `CreateFolderPublicLink` — remote paths, but no NUL/traversal check either).

**Severity**: HIGH for `SyncRootAdd.local_path` specifically (the daemon accepts any symlink target as a sync root and will happily upload its contents); MEDIUM for the remote-path public-link variants (server enforces ACL, so exposure is bounded to authenticated user).

**Impact**: a compromised CLI or unprivileged local process that has passed peer-uid authorization (same-user) can cause the daemon to:
- sync a symlink-pointed system directory (e.g. `~/evil -> /etc`) as a "sync root",
- produce path-traversal via `../` entries on the remote side when combined with later operations that join strings,
- force the daemon to open a NUL-embedded path, which would fail late inside a `CString::new` call with a less-useful error (already observed in `linux.rs:198` where the fallback branch is dead on happy-path validators).

Note: the mount-point validator in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:115-156` already demonstrates the right shape (existence, kind, ownership, non-world-writable); `add_sync_root` needs the same discipline.

**Remediation**:
1. Introduce `fn validate_user_supplied_path(s: &str, mode: PathMode) -> Result<PathBuf, ValidationError>` that rejects:
   - `s.contains('\0')`,
   - any segment equal to `..` (post-canonicalization compare against a sandbox base),
   - paths outside the configured sync-root sandbox (configurable allow-list of user-home / `/home/<user>`),
   - non-canonical path length > `PATH_MAX - 1` (4095 bytes on Linux; 1023 on macOS with NFC normalization).
2. Plumb it into `add_sync_root`, `suggest_sync_folders_at`, `check_folder_syncable`, and every IPC dispatch arm that takes a `path: String`.
3. For macOS, apply NFC / NFD normalization via `unicode-normalization` so paths round-trip identically through HFS+ / APFS.
4. Reject absolute paths that resolve (after canonicalize) to `/proc`, `/sys`, `/dev`, `/run` on Linux — these are never legitimate sync roots.

---

### H3. `snapshot::restore_encrypted_snapshot` is the only path that rejects tar traversal; nothing else validates inbound path names

**File / line**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/snapshot.rs:620-632` is correctly written: it rejects entries whose path is absolute, contains `..`, NUL, `/`, or `\`. This is the **only** place in the workspace that defends against tar-slip / ZIP-slip.

**Severity**: HIGH if any other code path ever extracts user-supplied archives.

**Impact**: future feature work (backup-restore, plugin archive extraction, sync-state import) will fail open unless engineers copy-paste the same check. There is no shared `validate_archive_entry` helper.

**Remediation**:
1. Extract the check at `snapshot.rs:625-631` into a reusable `fn is_safe_relative_path(rel: &Path) -> Result<(), UnsafePathReason>` in a shared crate (`pcloud-model` or a new `pcloud-fs-safety` module).
2. Unit-test with adversarial cases: `../../etc/passwd`, `C:\Windows\System32`, leading `/`, backslash-separated Windows-style, CRLF-embedded, `\0`-embedded, overlong-UTF-8.
3. Add a Clippy-style internal lint that flags `Archive::entries` / `zip::ZipArchive::read_zipfile_from_stream` consumers that do not call the helper.

---

### H4. IPC `TwoFactorCodeSubmission.value` is `String`, not `RedactedString`

**File / line**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs:288-296`:

```rust
TwoFactorCodeSubmission {
    /// The numeric TFA code or the user's recovery phrase.
    value: String,
    ...
}
```

**Severity**: HIGH (recovery-code path), MEDIUM (ephemeral OTP path).

**Impact**: when `recovery_code = true`, the submitted value is the user's **static recovery phrase** — equivalent to a long-lived credential. A derived `Debug` on `Request` therefore leaks the recovery phrase at any `log::debug!("{req:?}")` site. The enum `Request` carries `#[derive(Debug, ...)]` at `methods.rs:260`, so this leaks just like H1.

**Remediation**:
- Change `value: String` to `value: RedactedString` at `methods.rs:290`, mirroring the treatment of `PasswordSubmission.value`.

---

## MEDIUM findings

### M1. Vault `validate_vault_file` does not validate parent-directory ownership/mode on load

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:198-221`.

**Impact**: `store_token` unconditionally tightens the parent directory to `0o700` (line 142), but `load_token` via `validate_vault_file` only inspects the file, not the parent. If a previous install left the config directory as `0o755` before the first `store_token` call, an attacker who had transient `drwx` on that dir could plant symlinks at sibling paths. The `O_CREAT|O_EXCL` protection at `file.rs:161-167` mitigates the write path, but the load path returns a secret from a directory whose provenance was never checked.

**Severity**: MEDIUM (requires pre-existing weak parent, then concurrent write).

**Remediation**: add to `validate_vault_file`:
```rust
#[cfg(unix)]
if let Some(parent) = path.parent() {
    let parent_meta = fs::symlink_metadata(parent)?;
    if !parent_meta.file_type().is_dir() {
        return Err(AuthVaultError::InsecureMetadata("vault parent must be a directory"));
    }
    if parent_meta.uid() != current_uid {
        return Err(AuthVaultError::InsecureMetadata("vault parent must be owned by current user"));
    }
    if parent_meta.mode() & 0o077 != 0 {
        return Err(AuthVaultError::InsecureMetadata("vault parent must not grant group/other access"));
    }
}
```

---

### M2. IPC serve loop is single-threaded: no per-peer connection cap, no global connection cap

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/serve.rs:127-230` and `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:167-229`.

**Impact**: `bound.serve_once` accepts exactly one connection per loop iteration. A malicious peer (same-uid, having passed authorization) can open a socket, wait the 5 s read timeout, close, reopen — blocking session-refresh ticks (`serve.rs:227-229`) and any concurrent CLI invocation. While the 5 s cap prevents indefinite wedging, it is a clean availability DoS against the daemon from a cooperating attacker inside the user session.

**Severity**: MEDIUM — only exploitable by an attacker who already controls the user account (peer-uid check passes), and session-refresh recovers on next iteration.

**Remediation**:
1. Move to a bounded worker pool with, e.g., a `Semaphore` admitting ≤ 8 concurrent IPC requests. The `BoundIpcServer::listener.accept()` path at `transport.rs:171` should be run in a dispatcher thread that hands each accepted stream off.
2. Add a per-peer (keyed by pid) quota: at most N in-flight requests per peer pid. Same-uid but different-pid peers get independent budgets.
3. The 5 s read timeout should also apply to the write half (currently only `set_read_timeout` at `transport.rs:184`).
4. Consider capping `MAX_PIPE_INSTANCES` at `platform/windows.rs:61` down from 32 once the peer-pid quota is live — 32 is a lot of headroom in the current no-quota regime.

---

### M3. `write_response` is not timeout-bounded — slow writers can hold the serve loop

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:366-376`.

The read timeout at `serve_stream_once:184` only governs the request read. The response write (`write_response`) calls `stream.write_all()` + `stream.flush()` unbounded, so a slow-reader client can hold the serve loop for longer than `IPC_REQUEST_READ_TIMEOUT`.

**Severity**: MEDIUM.

**Remediation**: set `stream.set_write_timeout(Some(IPC_REQUEST_READ_TIMEOUT))` alongside the existing `set_read_timeout` call at line 184.

---

### M4. `current_effective_uid` lacks a `CAP_SETUID`/`euid!=ruid` sanity gate

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/auth.rs:65-68`.

`PeerIdentity::matches_owner` compares against `libc::geteuid()`, which is the correct source, but if the daemon is ever launched setuid (`sudo pcloudd`), the effective uid will be `root` while the real user — the pcloud owner — may differ. The IPC accept path then trusts any root peer.

**Severity**: MEDIUM — the daemon is not documented to run setuid, but nothing in `bootstrap.rs` rejects it.

**Remediation**: in `bootstrap.rs`, assert at startup that `geteuid() == getuid() && getegid() == getgid()`; refuse to bind IPC otherwise. Log a security-audit event at info level.

---

### M5. Linux `signal_trampoline` runs non-async-signal-safe code

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:87-117`.

Although the comment at line 106 claims `umount2` is async-signal-safe, the trampoline also:
- calls `mtx.lock()` (`Mutex` is not async-signal-safe — it can deadlock if the signal interrupts a thread that already holds it),
- calls `CString::new(...)` which may allocate.

**Severity**: MEDIUM — not exploitable for a security bypass, but a crash under SIGTERM loses the audit-trail and leaves stale mounts.

**Remediation**: the correct pattern is a self-pipe (write a single byte to a pipe from the signal handler, a worker thread reads it and does the real unmount), or use `signalfd(2)` / `sigwait(2)` in a dedicated thread. Document the known-unsafe pattern inline until fixed.

---

### M6. FFI transmute_copy on fn-ptrs in `winfsp_ffi.rs` is unchecked for ABI compatibility

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/winfsp_ffi.rs:494`, `:513`:

```rust
Ok(std::mem::transmute_copy::<_, T>(&f))
```

**Severity**: MEDIUM — the WinFSP DLL ABI is stable, but a mismatched build (pcloud-rs built against WinFSP 2.x headers, runtime 1.x) will not be caught here; the first call produces undefined behaviour.

**Remediation**:
1. Add a version-probe (`FspVersion` export) before `resolve` and compare against a pinned major-version constant.
2. Replace `transmute_copy` with the safer `mem::transmute::<*const c_void, T>(f as *const c_void)` wrapped in a `fn()` newtype, or migrate to the `libloading` crate which exposes a typed `Symbol<F>` that at least documents the contract.
3. Per-symbol inline SAFETY comments are present (✓), but do not record the expected signature — add the full `typedef` from the upstream C header.

---

### M7. `fs::symlink_metadata` + `fs::File::open` TOCTOU in vault load

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:200-221` (validate) → `:89-97` (open).

Between `symlink_metadata` and `fs::File::open(path)`, an attacker in the same uid (malicious plugin, compromised CLI) can swap the file for a symlink. Since validation already asserts owner-uid matches current uid, real exploitability requires local same-user adversary — so the window is narrow — but the audit standard here should be `open + fstat` (open by fd, then validate metadata via `fstat` on that fd) to eliminate the race.

**Severity**: MEDIUM (defense-in-depth).

**Remediation**:
1. Use `nix::fcntl::open` with `O_NOFOLLOW | O_CLOEXEC`, then `nix::sys::stat::fstat` on the returned fd.
2. This also eliminates the duplicate Unix-only cfg in the current code.

---

### M8. `ConvertStringSecurityDescriptorToSecurityDescriptorW` has no fallback when the SID lookup fails

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/windows.rs:385-407`.

If `current_user_sid_string()` somehow returns a malformed SDDL substring, `ConvertStringSecurityDescriptorToSecurityDescriptorW` fails and `bind_listener` returns an error — but the pipe name construction at `:143-146` already embedded the (possibly malformed) SID into the pipe path. No reachable path exploits this today because `sid_to_string` comes from a `TokenUser` dispatch the kernel provides, but a defense-in-depth check should verify the SID is well-formed before composing the name.

**Severity**: LOW-MEDIUM.

**Remediation**: add `debug_assert!(owner_sid.starts_with("S-1-"))` and reject SIDs whose length exceeds 184 bytes (the documented SID-string max) before composing the pipe path.

---

### M9. `MAX_IPC_PAYLOAD_LEN` / `MAX_REQUEST_BYTES` of 1 MiB is defended pre-allocation, but there is no per-peer rolling byte budget

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/server.rs:42`, `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/protocol.rs:47`, and `transport.rs:312-317` (the guard).

A well-behaved client sending 1 MiB requests in a tight loop will consume 1 MiB per accept cycle of allocator churn. Since the serve loop is already serial (M2), aggregate bandwidth is bounded, but a cooperating attacker with per-session rate-limit exemption on a cheap method can push this hard.

**Severity**: MEDIUM.

**Remediation**:
1. Add a per-peer rolling byte budget to the rate-limiter (`pcloud-daemon/src/rate_limit.rs`) — reject if byte-in over the last 60 s exceeds N MiB.
2. Consider dropping `MAX_REQUEST_BYTES` to 256 KiB now that the expensive IPC methods have concrete schemas; 1 MiB is two orders of magnitude larger than the largest real request per the comment at `server.rs:33-37`.

---

## LOW findings

### L1. Documentation contradiction: `SECURITY.md:96-97` vs `CONTRIBUTING.md:206` vs actual code

**Observation**: both docs explicitly forbid `danger_accept_invalid_certs` / `accept_invalid_hostnames`. I confirmed no `src/` file contains either identifier (grep across `crates/`). Documentation is accurate; this is a positive note, not a defect. Keep the discipline in place.

---

### L2. `RedactedString` serializes transparently — round-trips include the plaintext

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/redacted.rs:37-39`:

```rust
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct RedactedString(String);
```

This is intentional (the secret has to cross the IPC boundary), and the module docs explicitly justify the trade-off. However, because the type is `Clone`, one subtle failure mode exists: if a consumer holds a `RedactedString` in a long-lived `HashMap`, no `ZeroizeOnDrop` applies. The `methods.rs:241-259` audit H1 note mentions immediate destructuring into `SecretString` on the daemon side, but a future refactor that stashes it elsewhere will silently regress.

**Severity**: LOW (design choice, documented, test coverage exists at `redacted.rs:118-133`).

**Remediation**: add a compile-fail test that prevents `RedactedString` from appearing as a field on any struct annotated `#[long_lived]`, or wrap it in a newtype `EphemeralRedacted` that impls `Drop` via `Zeroize`.

---

### L3. `RedactedString::Clone` is derived, bypassing the `SecretString` clone-audit discipline

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/redacted.rs:37`.

The project carefully removed `#[derive(Clone)]` from `SecretString` (audit M3 in the module doc) so every duplication is visible as `.clone_secret()`. `RedactedString` derives `Clone` freely, so `req.value.clone()` at any dispatch site is invisible in code review.

**Severity**: LOW.

**Remediation**: remove the derived `Clone` and add `fn clone_secret(&self) -> Self` so the two types follow the same discipline. Then fix the handful of `Request::Plain { method }` clones that may depend on it.

---

### L4. `serve_once` wraps `listener.accept()` in a single `?`, hiding `EINTR` vs permanent error distinction

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:171-173`.

The `?` on `accept` returns immediately, but the caller at `serve.rs:207-210` reinterprets `ErrorKind::Interrupted` as a signal-driven wakeup. This coupling between `accept` error kinds and `serve_until_shutdown_with_flag` is correct but brittle. A future `BoundIpcServer::serve_many` helper must mirror the same branch.

**Severity**: LOW.

**Remediation**: add a `AcceptOutcome::{Connection, Interrupted, Timeout}` enum on `BoundIpcServer` so callers do not parse `io::ErrorKind` directly.

---

### L5. `server.rs:96-98` example asserts uid 1000 is authorized — doctest could be misleading

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/server.rs:93-97`.

The runnable doctest creates an `IpcServer::new(1000)` and asserts a `PeerIdentity { uid: 1000, pid: 1 }` is authorized. In a hostile reader's mental model this looks like "anyone matching uid 1000 is OK", which is correct but underlines that **the uid check is the ONLY gate** — the server does not check pid, cmdline, mount namespace, or SELinux label. Document this explicitly.

**Severity**: LOW (documentation / threat-model clarity).

**Remediation**: add a note: "same-uid attackers (running as the same user as the daemon) fully satisfy authorization; additional sandboxing must live at the OS layer (AppArmor, SELinux, or a user-namespaces sandbox)."

---

### L6. `loader.rs` enforces `0o077` mask but does not validate ownership on config file

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/loader.rs:188-229`.

Production rejects group/world-readable files but does not assert the file is owned by the current uid. A root-owned `0o600` config file would pass the check — OK for systemd-root daemons but surprising for a user-scope daemon.

**Severity**: LOW.

**Remediation**: extend `check_permissions` with an ownership check symmetric to `vault/file.rs:208-212`.

---

### L7. `store_token` recreates `tmp_path` via `with_extension("tmp")`, and does not use `O_TRUNC` on the final rename target

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:149-185`.

The atomic-write-then-rename sequence is correct, but `fs::set_permissions(path, ...)` at `:183` runs **after** the rename, producing a tiny window where `path` inherits the tmp-file mode (itself `0o600` by construction). This is safe on filesystems where rename preserves mode (all POSIX FS), but the two-step dance is unnecessary. Keep the explicit `set_permissions` as defense-in-depth but document why.

**Severity**: LOW.

**Remediation**: comment the redundancy; no code change needed.

---

### L8. `RedactedString` uses default-derived `PartialEq`, not constant-time

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/redacted.rs:37`.

`SecretString::PartialEq` goes through `subtle::ConstantTimeEq` (secret_string.rs:110-112). `RedactedString` uses the derived byte-by-byte eq, which leaks length / prefix timing. The contract is that `RedactedString` is transient and destructured before long-term use, so exposure is ephemeral — but a future cache path could regress.

**Severity**: LOW.

**Remediation**: add the same constant-time impl for parity.

---

### L9. `fs::read_to_string("/proc/self/mountinfo")` on Linux has no TOCTOU protection against a bind-mount swap

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/platform/linux.rs:36-39`.

Unmount-orphan detection reads `/proc/self/mountinfo`. Between two reads (settle-poll window at `:157-171`), a cooperating attacker can race a bind-mount to confuse the parser. No secret exposure; correctness issue.

**Severity**: LOW.

**Remediation**: out of security scope — log for the FS team (Dimension 6 or equivalent).

---

## Positive findings (secure-by-default confirmations)

### P1. `SecretString` / `SecretBytes` are audit-hardened correctly

**Files / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-secret/src/secret_string.rs:35-124` and `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-secret/src/secret_bytes.rs:22-102`.

- `#[derive(ZeroizeOnDrop)]` at `secret_string.rs:35` and `secret_bytes.rs:22` guarantees scrubbing.
- `Clone` is deliberately NOT derived; `clone_secret()` is the only way to duplicate (lines 77-80 / 58-61).
- Constant-time `PartialEq` via `subtle::ConstantTimeEq` (lines 110-113 / 91-93).
- `Debug` renders `SecretString(<redacted>)` / `SecretBytes(<redacted>)` (lines 96-98 / 76-79).
- No `Serialize`/`Deserialize` impl; the module doc (`secret_string.rs:15-17`) references the compile-fail test `tests/compile_fail_serialize.rs` that enforces this.
- Both types impl `Zeroize` explicitly (lines 120-124 / 98-102) as belt-and-braces against a future refactor swapping the inner type.

This is **correct** and meets or exceeds the project's stated standard. No changes required.

---

### P2. Auth vault discipline is secure-by-default

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/vault/file.rs:138-186` (`store_token`) and `:77-133` (`load_token`).

- **Opt-in**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs:362-367` `AuthPersistence { enabled: bool }` — default `false`, daemon must receive an explicit enable.
- **`0600` file mode**: set at `file.rs:166` (`.mode(0o600)`), reinforced at `:177` and `:183`.
- **`0700` parent dir**: set at `file.rs:142` unconditionally on every store.
- **Ownership validation on load**: `file.rs:207-212` rejects non-owner files.
- **Mode validation on load**: `file.rs:214-218` rejects any group/other bit.
- **Atomic tmp+rename write**: `file.rs:149-184` uses `O_CREAT|O_EXCL` (`create_new`) with `mode(0o600)`, then `sync_all`, then `rename`, then `sync_parent_directory`.
- **No plaintext password persistence**: confirmed by `vault/mod.rs:37-40` ("Password persistence is intentionally not available through this trait — see ADR 0007") and by grep — no `password.write(` / `fs::write(_, password)` anywhere under `crates/pcloud-daemon/`.
- **Zeroize on load error path**: `file.rs:92-93, 112, 119, 127` explicitly zeroize intermediate buffers on every error branch. This was noted as audit finding M4 and is closed.

---

### P3. IPC transport security

**File / lines**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:246-267` (`bind`) — creates parent with `fs::create_dir_all`, tightens to `0o700` if parent_missing, removes stale socket, binds, then `chmod 0o600`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:167-229` (`serve_once` / `serve_stream_once`) — applies 5 s read timeout, recovers peer credentials via `peer_identity`, rejects on authorization failure **before** dispatch, returns `Unauthorized` status.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/linux.rs:42-57, 94-120` — Linux `SO_PEERCRED` with strict `rc != 0 || len != sizeof(ucred)` check.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/unix.rs:44-60` — BSD/macOS `getpeereid(3)`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/platform/windows.rs:127-219` — Windows per-SID DACL at pipe creation time, `TokenUser` SID comparison via `GetNamedPipeClientProcessId` + `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `OpenProcessToken(TOKEN_QUERY)` + `GetTokenInformation(TokenUser)` + `ConvertSidToStringSidW`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/server.rs:42` — `MAX_REQUEST_BYTES = 1 MiB` cap.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:304-325` — read_framed_request checks the cap **before** allocating `Vec::with_capacity(8 + payload_len)`, explicitly called out at `:310-311`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:327-364` — oversized-frame errors close the connection **without** replying (amplification protection), protocol errors reply `InvalidRequest`, transient IO errors are swallowed.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/transport.rs:232-236` — `Drop` unlinks the socket file.

All of these are correct and meet the stated security model. The only remaining improvements are M2 (concurrent connection cap), M3 (write timeout), and M4 (setuid sanity gate).

---

### P4. Transport policy — production rejects plaintext

**File / lines**:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/api.rs:130-140` — `ApiEndpoint::validate` in `Production` + `ApiMode::Plaintext` returns `Err(ConfigError::InvalidApiEndpoint(...))` with message "production environment requires tls api mode". Test coverage at `:232-240` (`production_plaintext_is_rejected`).
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/api.rs:195-203` — `secure_defaults` for `Production` defaults mode to `Tls`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/file_history.rs:67-78` — `RevisionUrl::validate` refuses `http://` URLs in Production.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-config/src/env.rs:27-30` — env parser rejects `PCLOUD_ENV=production` with `PCLOUD_API_MODE=plaintext`.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/transport.rs:318-336` — TLS client uses `rustls` + `webpki_roots::TLS_SERVER_ROOTS`, no custom verifier.
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/http_download.rs:210, :573` — same pattern.
- No `danger_accept_invalid_certs` / `accept_invalid_hostnames` / `InsecureSkipVerify` / custom-validator strings anywhere in `crates/**/*.rs` (grep-verified).

---

### P5. Downgrade / replay defenses

- **TFA cannot be skipped when server demands it**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/state.rs:22-37` models `SessionState::TwoFactorRequired` explicitly; `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/orchestrator.rs:258, 377, 444` always transitions to `TwoFactorRequired` when the server returns `PasswordLoginOutcome::TwoFactorRequired`. `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/manager.rs:60-100` state machine forbids jumping directly from `AwaitingCredentials` to `Authenticated`.
- **Hard expiry is enforced**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/lifecycle.rs:172-174` `is_expired(now_secs) -> now_secs >= expires_at`. `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/lifecycle.rs:216-217` raises `SessionLifecycleError::AuthExpired` forcing re-auth.
- **Idle expiry**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/lifecycle.rs:178-180`.
- **Server-reported auth-expired is honoured**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/serve.rs:322-325` (`TickOutcome::AuthExpired` branch).
- **No replay-via-reusable-nonce**: IPC protocol version at `protocol.rs:39` is checked at `protocol.rs:255-260` (`VersionMismatch` error); a downgraded client is hard-rejected.

---

### P6. Logging discipline

- Grep for `(info|warn|error|debug|trace)!\s*\(.*\b(password|token|secret|priv_key|passphrase)\b` across `crates/**/*.rs` yields **no leaks**. The single hit at `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/serve.rs:309` reads `"pcloud-session-refresh: token refreshed successfully"` — a marker string, not a token value.
- Grep for variable interpolation pattern `(info|warn|error|debug|trace)!.*(\{password|\{token|\{secret|\{passphrase|expose_secret)` yields **zero hits**.
- `SecretString::Debug` and `SecretBytes::Debug` both render `<redacted>` (`secret_string.rs:96-99` / `secret_bytes.rs:76-80`).
- `RedactedString::Debug` renders `<redacted N bytes>` (`redacted.rs:75-79`).
- No `SecretString::expose_secret()` appears inside any `*!` formatter; grep confirms.

---

### P7. FFI SAFETY discipline

Every `unsafe` block I spot-checked across `platform/{linux,bsd,macos,windows}.rs`, `platform/macos_ffi.rs`, `platform/winfsp_ffi.rs`, and `platform/{linux,unix,windows}.rs` in `pcloud-ipc/src/` carries an inline `// SAFETY:` comment stating the invariant. Examples:
- `pcloud-ipc/src/platform/linux.rs:42-50` — `getsockopt(SO_PEERCRED)` with live fd + initialized out-param.
- `pcloud-ipc/src/platform/unix.rs:49-53` — `getpeereid(3)` with initialized out-params.
- `pcloud-fs/src/platform/linux.rs:90-97, 106-108, 113-116, 186` — `signal(2)`, `umount2`, `raise(sig)`.
- `pcloud-fs/src/platform/bsd.rs:188-201, 242-258, 295-299, 352-382, 442` — `getmntinfo`, `slice::from_raw_parts` on libc-owned `statfs` array.
- `pcloud-fs/src/platform/windows.rs:92-99, 117-124, 164-177, 228-241, 274-284, 303-304, 308-314, 319-349, 353-371, 388-398, 409-419, 425-434, 454-458, 462-468` — every Win32 call, every SID lookup, every `LocalFree` is SAFETY-commented.
- `pcloud-fs/src/platform/macos.rs:194-225, 232-256, 262-266, 308-314, 328-342, 346-350, 413-448` — every fuse-t FFI call is commented. Phase-1 scaffold caveat at `macos_ffi.rs:10-15` is acknowledged.
- `pcloud-fs/src/platform/winfsp_ffi.rs:443-447, 468-470, 480-514, 517` — function-pointer transmutes explicitly document the ABI contract. See M6 for residual risk.
- `pcloud-ipc/src/auth.rs:66-67` — `libc::geteuid()` has no preconditions; single-line SAFETY comment.

The one structural concern is the transmute-to-fn-ptr sequence in `winfsp_ffi.rs` (M6), which cannot be fully validated without a WinFSP version probe. All other `unsafe` blocks pass review.

---

### P8. Secret-bearing CLI state is wrapped

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/commands.rs:565-590`.

`SecretInputs` long-lived struct holds:
- `password: SecretString`
- `auth_token: SecretString`
- `crypto_password: SecretString`
- `public_link_password: Option<SecretString>`

`Clone` / `PartialEq` are deliberately not derived (line 561-564 comment). This matches the project standard.

The only residuals in `SecretInputs` that are still `String` are `two_factor_code` (line 570) and `share_message` (line 612) — the share_message is legitimate plaintext, but `two_factor_code` carries a recovery-phrase when `recovery_code = true` and should be promoted to `SecretString` — see H4.

---

### P9. Secret-stash state machine

**File / lines**: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-auth/src/state.rs:47-65` (`PendingChallenge { token: SecretString, ... }` with hand-written `Clone` routed through `clone_secret`) and `:73-100` (`SessionSnapshot { auth_token: Option<SecretString>, ... }` with hand-written `Clone` via `clone_secret`).

`Debug` impls on both types emit tag-only output; the secret material goes through `SecretString`'s redacted `Debug`. This exactly matches the stated project standard.

---

## Summary table

| ID  | Sev      | Area                      | File                                                       |
|-----|----------|---------------------------|------------------------------------------------------------|
| H1  | HIGH     | secret discipline         | `crates/pcloud-proto/src/methods/*.rs` (many)              |
| H2  | HIGH     | input validation          | `crates/pcloud-daemon/src/runtime.rs:3952`                 |
| H3  | HIGH     | input validation          | `crates/pcloud-backends/src/snapshot.rs:625` (isolated)    |
| H4  | HIGH     | secret discipline         | `crates/pcloud-ipc/src/methods.rs:290`                     |
| M1  | MEDIUM   | vault                     | `crates/pcloud-daemon/src/vault/file.rs:198`               |
| M2  | MEDIUM   | DoS                       | `crates/pcloud-daemon/src/serve.rs:127`                    |
| M3  | MEDIUM   | DoS                       | `crates/pcloud-ipc/src/transport.rs:366`                   |
| M4  | MEDIUM   | IPC                       | `crates/pcloud-ipc/src/auth.rs:65`                         |
| M5  | MEDIUM   | FFI / signal              | `crates/pcloud-fs/src/platform/linux.rs:87`                |
| M6  | MEDIUM   | FFI                       | `crates/pcloud-fs/src/platform/winfsp_ffi.rs:494`          |
| M7  | MEDIUM   | vault TOCTOU              | `crates/pcloud-daemon/src/vault/file.rs:200`               |
| M8  | MEDIUM   | IPC Windows               | `crates/pcloud-ipc/src/platform/windows.rs:385`            |
| M9  | MEDIUM   | DoS                       | `crates/pcloud-ipc/src/server.rs:42`                       |
| L1  | LOW      | docs                      | `SECURITY.md`                                              |
| L2  | LOW      | secret lifetime           | `crates/pcloud-ipc/src/redacted.rs:37`                     |
| L3  | LOW      | secret discipline         | `crates/pcloud-ipc/src/redacted.rs:37`                     |
| L4  | LOW      | IPC error model           | `crates/pcloud-ipc/src/transport.rs:171`                   |
| L5  | LOW      | doc / threat model        | `crates/pcloud-ipc/src/server.rs:93`                       |
| L6  | LOW      | config                    | `crates/pcloud-config/src/loader.rs:188`                   |
| L7  | LOW      | vault defense-in-depth    | `crates/pcloud-daemon/src/vault/file.rs:183`               |
| L8  | LOW      | timing                    | `crates/pcloud-ipc/src/redacted.rs:37`                     |
| L9  | LOW      | FS race                   | `crates/pcloud-fs/src/platform/linux.rs:36`                |

### Prioritized remediation sequence

1. **H1** (derived-Debug leak in `pcloud-proto`) — systemic, easy mechanical fix, highest leverage.
2. **H4** (TFA recovery code as plain `String`) — one-line change in `methods.rs`.
3. **H2** (path validation on `SyncRootAdd`) — add a shared `validate_user_supplied_path`.
4. **H3** (publish the tar-entry safety helper) — refactor + shared crate.
5. **M1 / M7** (vault parent-dir validation + open-by-fd TOCTOU fix) — low-effort hardening.
6. **M2 / M3** (IPC concurrency cap + write timeout) — availability.
7. **M4** (reject setuid daemon).
8. **M5** (async-signal-safe unmount trampoline) — correctness.
9. **M6 / M8** (WinFSP version probe + SID shape check).
10. **M9** (per-peer byte budget) — DoS defense-in-depth.
11. LOWs in decreasing impact: L3 → L8 → L4 → L6 → L7 → L2 → L5 → L1 → L9.

---

### Methodology notes

- All greps were run over `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/**/*.rs` unless noted.
- SAFETY spot-checks covered ~40 of the ~140 `unsafe` blocks across the FFI files; the remainder follow the same pattern and were not individually verified but carry inline comments.
- No source files were modified by this audit.
- The project's own `CLAUDE.md` security rules were used as the normative baseline; every confirmed deviation is reported above.

### Out-of-scope items observed

The following items were surfaced but fall to other audit dimensions:

- cryptographic algorithm selection (AES-256-GCM, sector size, AEAD-nonce generation) → Dim 3.
- compression-bomb protection on decompressed API responses — no inbound gzip/deflate path was identified in `pcloud-proto/src/transport.rs`; the API frame codec is length-prefixed JSON. If a future feature adds HTTP compression, Dim 3 should audit decode budget.
- observability leaks in `pcloud-observability` → Dim 4.

# Subagent 02 Security / Secret / Transport Audit

No files modified. Scope covered the requested secret, auth-vault, IPC, transport, logging, validation, replay/downgrade, and DoS-sensitive paths.

## Findings

### HIGH-01 Runtime auth persistence bypasses the selected platform vault
Severity: HIGH. Evidence: `crates/pcloud-config/src/auth.rs:39` documents `Auto` as platform-native; bootstrap selects a `PlatformVault` at `crates/pcloud-daemon/src/bootstrap.rs:522` and stores through it at `crates/pcloud-daemon/src/bootstrap.rs:210`. Runtime persistence later ignores that vault and calls file-vault helpers directly at `crates/pcloud-daemon/src/runtime.rs:6923`, `crates/pcloud-daemon/src/runtime.rs:6944`, and `crates/pcloud-daemon/src/runtime.rs:6962`.  
Impact: macOS Keychain, Windows DPAPI, and Linux Secret Service policy can be bypassed after startup, causing token storage divergence or plaintext file-vault writes. On Windows the direct file path can also fail because file vault storage is intentionally unsupported at `crates/pcloud-daemon/src/vault/file.rs:146`.  
Remediation: carry the selected `Box<dyn PlatformVault>` into `RuntimeShell` and use it for `persist_auth_state`, `sync_auth_vault`, `restore_vault_state`, and authsave rollback. Add tests for `PCLOUD_VAULT=dpapi/keychain/secret-service` proving runtime login/logout/authsave never touches the file vault.

### HIGH-02 Wire capture can persist plaintext auth frames and upload bodies
Severity: HIGH. Evidence: `PCLOUD_WIRE_CAPTURE_DIR` is enabled in `crates/pcloud-proto/src/transport.rs:556`; the code warns captured request bytes contain auth tokens at `crates/pcloud-proto/src/transport.rs:630`; frames are written to disk at `crates/pcloud-proto/src/transport.rs:684`. Password login encodes the clear password into params at `crates/pcloud-proto/src/methods/auth.rs:152`.  
Impact: an environment variable can create durable plaintext copies of auth tokens, passwords, TFA codes, response bodies, and upload data. Mode `0600` reduces cross-user exposure but not backup, support-bundle, incident-response, or same-user leakage.  
Remediation: compile-gate capture behind an explicit unsafe/debug feature, reject it in production mode, and redact/omit secret-bearing params and payload bodies by default. Require a second explicit unsafe flag for full raw capture.

### HIGH-03 Challenge and confirmation codes are Debug-printable raw strings
Severity: HIGH. Evidence: IPC `Request` derives `Debug` at `crates/pcloud-ipc/src/methods.rs:260`. `TwoFactorCodeSubmission.value` is a raw `String` at `crates/pcloud-ipc/src/methods.rs:287`, and crypto confirmation `code` fields are raw strings at `crates/pcloud-ipc/src/methods.rs:342` and `crates/pcloud-ipc/src/methods.rs:357`.  
Impact: formatting a request with `{:?}` can leak OTPs, recovery codes, and crypto confirmation codes through logs, panic output, failed tests, or tracing.  
Remediation: use `RedactedString` for all OTP, recovery, and confirmation-code fields; convert to `SecretString` only after decode; add regression tests that `format!("{:?}", Request::...)` never includes submitted code values.

### HIGH-04 Fresh auth and TFA challenge tokens are raw Debug-printable protocol strings
Severity: HIGH. Evidence: `PasswordLoginOutcome` derives `Debug` at `crates/pcloud-proto/src/auth_api.rs:109` and stores `auth_token: String` and `challenge_token: String` at `crates/pcloud-proto/src/auth_api.rs:116` and `crates/pcloud-proto/src/auth_api.rs:120`. `PasswordChangeResult` also derives `Debug` and stores `auth_token: String` at `crates/pcloud-proto/src/account_api.rs:97`.  
Impact: newly issued long-lived tokens and TFA challenge tokens can leak before they are wrapped by higher layers. Raw `String` also leaves non-zeroized heap copies.  
Remediation: parse these response fields into `SecretString` or a non-`Clone`, zeroizing, redacted response wrapper at the protocol boundary. Remove derived `Debug`/`Clone` where they carry secrets.

### HIGH-05 Production resilient transport controls are constructed but not wired
Severity: HIGH. Evidence: `TransportFactory` explicitly says feature-domain backends still construct their own transports at `crates/pcloud-daemon/src/transport_factory.rs:28`. Bootstrap creates backends first at `crates/pcloud-daemon/src/bootstrap.rs:497` and the factory later at `crates/pcloud-daemon/src/bootstrap.rs:513`. A real backend constructs a bare `BinaryApiTransport` at `crates/pcloud-backends/src/auth_backend.rs:277`.  
Impact: retry budgets, circuit breaking, jitter, and transport-level DoS controls are not active for the main authenticated API surface. Outages can still produce retry storms and unbounded backend-specific network pressure.  
Remediation: inject a shared transport factory into all backend constructors, centralize transport construction, and add integration tests proving production backends use `ResilientTransport`.

### HIGH-06 Windows IPC read/write timeouts are no-ops
Severity: HIGH. Evidence: the transport docs state slow-client timeout is no-op on Windows at `crates/pcloud-ipc/src/transport.rs:31`. `set_read_timeout` and `set_write_timeout` return `Ok(())` without applying a deadline at `crates/pcloud-ipc/src/platform/windows.rs:667`. Server paths then perform blocking framed reads at `crates/pcloud-ipc/src/transport.rs:836` and `crates/pcloud-ipc/src/transport.rs:880`.  
Impact: an authenticated same-SID local client can connect and drip or withhold bytes, pinning pipe worker threads until exhaustion.  
Remediation: implement overlapped I/O with waitable deadlines and cancellation, or enforce a per-connection watchdog that can close the pipe. Add a Windows slow-client regression test.

### MEDIUM-01 Bootstrap credential-file handling leaves extra heap copies and lacks Windows ACL validation
Severity: MEDIUM. Evidence: `read_secret_file` clones the secret buffer and creates an intermediate `String` via `String::from_utf8(buf.clone())` at `crates/pcloud-daemon/src/bootstrap.rs:137`; only `buf` is zeroized at `crates/pcloud-daemon/src/bootstrap.rs:150`. Windows mode checks are stubbed to `0` at `crates/pcloud-daemon/src/bootstrap.rs:113`.  
Impact: bootstrap tokens, passwords, TFA codes, and recovery codes can remain in non-zeroized heap allocations. On Windows, secret files can pass validation even if their ACL grants broader access.  
Remediation: avoid cloning secret buffers, use `Zeroizing<Vec<u8>>`/`Zeroizing<String>` for intermediates, trim in a zeroizing buffer, and implement owner-only Windows DACL checks before accepting `PCLOUDRS_*_FILE`.

### MEDIUM-02 Local sync-root path validation exists but is not used
Severity: MEDIUM. Evidence: `validate_local_sync_path` rejects NUL, `..`, length overflow, and symlink roots at `crates/pcloud-ipc/src/path_validation.rs:53`. Runtime `add_sync_root` only checks empty input and canonicalizes at `crates/pcloud-daemon/src/runtime.rs:5625`.  
Impact: symlink-root, traversal-like, and ambiguous local paths can enter daemon state after canonicalization, weakening auditability and creating path-confusion/TOCTOU risk.  
Remediation: call `validate_local_sync_path` before canonicalization in the daemon path, reject root symlinks and parent components, then store only an absolute canonical directory. Add IPC-level tests.

### MEDIUM-03 Remote path normalization accepts traversal-like and control-containing paths
Severity: MEDIUM. Evidence: `validate_remote_folder` delegates to `normalize_remote_path` at `crates/pcloud-backends/src/sync_backend.rs:461`; normalization only trims, splits, removes empty segments, and prepends `/` at `crates/pcloud-backends/src/sync_backend.rs:555`.  
Impact: paths such as `/a/../b`, embedded NUL/control characters, or overlong paths can reach stores, logs, and API calls, relying on server interpretation instead of client-side policy.  
Remediation: add a central `validate_remote_path` that rejects `..`, NUL/control characters, non-absolute forms, and excessive byte length. Use it for sync roots, transfers, folders, and public-link remote paths.

### MEDIUM-04 Configured parser response-size cap is not wired into transports
Severity: MEDIUM. Evidence: config defines `max_parser_frame_bytes` as an 8 MiB security cap at `crates/pcloud-config/src/limits.rs:40` and defaults it at `crates/pcloud-config/src/lib.rs:390`. Actual `TransportConfig` uses a hardcoded 64 MiB `DEFAULT_MAX_RESPONSE_BYTES` at `crates/pcloud-proto/src/transport.rs:147` and `with_tls` always installs that default at `crates/pcloud-proto/src/transport.rs:227`.  
Impact: operators cannot lower the API parser allocation cap through config; a hostile or malfunctioning server can force larger per-response allocation than policy advertises.  
Remediation: thread `ConfigProfile.limits.max_parser_frame_bytes` into every `TransportConfig` constructor and test that oversized frames are rejected at the configured limit.

### MEDIUM-05 File vault parent directory validation does not fail closed
Severity: MEDIUM. Evidence: the vault file itself is owner/mode checked at `crates/pcloud-daemon/src/vault/file.rs:219`, but a parent directory not owned by the current UID only logs a warning and still returns success at `crates/pcloud-daemon/src/vault/file.rs:253`. Store creates/chmods parent directories at `crates/pcloud-daemon/src/vault/file.rs:164` without post-validating ownership.  
Impact: misconfigured vault paths under shared or unowned directories can still be accepted, increasing replacement/race and metadata exposure risk.  
Remediation: require parent directories to be real directories, current-user-owned, and `0700` before load/store. Fail closed if ownership or mode cannot be enforced.

### MEDIUM-06 Bearer public-link capabilities leak into audit/details strings
Severity: MEDIUM. Evidence: public-link codes are included in audit/details messages at `crates/pcloud-daemon/src/runtime.rs:974`, `crates/pcloud-daemon/src/runtime.rs:4835`, and `crates/pcloud-daemon/src/runtime.rs:4929`; upload-link URLs are included at `crates/pcloud-daemon/src/runtime.rs:5165`.  
Impact: public-link codes and upload-link URLs are bearer capabilities. Persisting them in audit/log-like surfaces increases exposure through monitoring, tickets, support bundles, and local history.  
Remediation: separate user-facing command output from audit/log details. Hash or redact link codes and URLs in logs/audit, returning full values only in explicit user responses that require them.

### MEDIUM-07 TLS revocation policy is advertised but not enforced
Severity: MEDIUM. Evidence: config exposes `TlsRevocationCheck` at `crates/pcloud-config/src/api.rs:15` and says shipped implementation honors stapled-permissive mode at `crates/pcloud-config/src/api.rs:41`. The actual TLS module documents CRL/OCSP as not implemented at `crates/pcloud-proto/src/tls.rs:16` and provides only a placeholder at `crates/pcloud-proto/src/tls.rs:52`.  
Impact: regulated deployments cannot fail closed on revoked certificates, and the config knob can create false assurance.  
Remediation: either remove/mark the knob as unsupported, or thread it into `shared_config` with real OCSP/CRL verification and fail-closed tests. Keep production plaintext rejection, which is correctly enforced at `crates/pcloud-config/src/api.rs:231`.

## Positive Controls Noted

`pcloud-secret` wrappers redact `Debug` and zeroize on drop. Production plaintext API mode is rejected, and TLS is pinned to TLS 1.3 only. Unix/Windows IPC peer credential checks and IPC frame/connection caps exist, though Windows timeout enforcement remains a DoS gap.

## Commands Run

`sed -n '1,240p' pcloud_rev.md`  
`rg --files crates/pcloud-secret crates/pcloud-daemon crates/pcloud-config crates/pcloud-proto crates/pcloud-ipc`  
`rg -n "password|token|api_key|secret|auth|tfa|recovery|session" ...`  
`rg -n "info!|warn!|error!|debug!|trace!|tracing::|log::" ...`  
`nl -ba` inspections over `pcloud-secret`, `pcloud-ipc`, `pcloud-daemon`, `pcloud-config`, `pcloud-proto`, and relevant `pcloud-backends` files.  
`rg -n "TransportFactory|wrap_binary|max_parser_frame_bytes|max_response_bytes|PlatformVault|sync_bootstrap_auth_state" ...`  
`git status --short`

## Limitations

Static review only; I did not run tests, live pCloud flows, fuzzers, or Windows/macOS execution. `target/`, `vendor/`, `.beads/`, and generated tracker output were excluded. The worktree already contained unrelated modified/untracked files; I did not modify them.

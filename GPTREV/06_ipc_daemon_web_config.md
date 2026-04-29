# Subagent 06 Audit: IPC, Daemon, Web, Config, Session/Auth

Scope covered `crates/pcloud-ipc`, `crates/pcloud-daemon`, `crates/pcloud-web`, `crates/pcloud-config`, `crates/pcloud-session`, `crates/pcloud-auth`, `crates/pcloud-daemon-win`, and related tests. I did not modify files or write `AUDIT_REPORT.md`.

## Findings

### HIGH-01: `SetApiServer` persists rejected API host hints and reports success
Severity: High

Evidence: `crates/pcloud-config/src/api.rs:292` rejects unsafe API hints, but `crates/pcloud-daemon/src/runtime.rs:2888` only logs that error, still applies the host to live runtimes at `crates/pcloud-daemon/src/runtime.rs:2901`, persists it at `crates/pcloud-daemon/src/runtime.rs:2907`, and the IPC handler reports success at `crates/pcloud-daemon/src/runtime.rs:3474`. The IPC method is documented as requiring no auth at `crates/pcloud-ipc/src/methods.rs:1204`.

Impact: Any same-UID IPC client can poison persisted API-server preference with an arbitrary host while receiving a successful response. Current lower transport validation reduces immediate redirection risk, but this violates config integrity and creates a dangerous future regression point.

Remediation: Make `RuntimeShell::set_api_server` return `InvalidRequest` immediately on rejected hints, do not update live runtimes or preferences, require authorization for API-region mutation, and add regression tests asserting invalid hosts leave config and runtime state unchanged.

### HIGH-02: Privileged IPC operations have no real authorization model beyond same-UID ownership
Severity: High

Evidence: IPC authentication is only peer-owner matching in `crates/pcloud-ipc/src/auth.rs:41`. `is_privileged_request` only logs classification in `crates/pcloud-daemon/src/serve.rs:101`, and privileged requests are still dispatched at `crates/pcloud-daemon/src/serve.rs:245`. Shutdown unconditionally flips the shutdown flag at `crates/pcloud-daemon/src/runtime.rs:3714`, while destructive/config-mutating dispatch is accepted in `crates/pcloud-daemon/src/runtime.rs:875`.

Impact: Same-user malware, compromised browser helpers, or untrusted automation can shut down the daemon, alter API settings, and trigger destructive account/runtime operations once local IPC is reachable.

Remediation: Add a capability layer separate from peer UID/SID. Gate shutdown, API-server mutation, backup/device deletion, mount force-unmount, crypto, sync, and public-link mutations behind explicit local admin grants or signed ephemeral tokens, and audit allow/deny decisions.

### HIGH-03: Web management read routes expose daemon state without the web token
Severity: High

Evidence: `/` reads daemon status without `require_web_token` at `crates/pcloud-web/src/routes.rs:132`; `/sync` calls `GetSyncRoots` and `GetPending` without token at `crates/pcloud-web/src/routes.rs:165`; `/publinks` lists public links at `crates/pcloud-web/src/routes.rs:252`; `/activity` lists notifications and can return JSON at `crates/pcloud-web/src/routes.rs:401`; `/settings` renders socket/config details at `crates/pcloud-web/src/routes.rs:436`. In contrast, `/api/status` is token-gated at `crates/pcloud-web/src/routes.rs:140`.

Impact: Loopback-only reduces exposure, but unauthenticated local web reads can leak sync roots, pending operations, public-link metadata, notification history, socket path, and status to same-host processes or browser-origin edge cases.

Remediation: Require the web token or a real authenticated web session on all daemon-backed routes except minimal health probes. Add Host allowlisting and Origin/Referer validation for browser flows.

### HIGH-04: Web token file creation is non-atomic and follows unsafe paths
Severity: High

Evidence: `write_web_token_to_runtime_dir` creates the runtime directory and ignores permission hardening errors at `crates/pcloud-web/src/lib.rs:316`; it then opens `web-token` with `create(true).truncate(true)` at `crates/pcloud-web/src/lib.rs:329` without `create_new`, temp-file rename, `O_NOFOLLOW`, owner/mode verification, `sync_all`, or parent sync.

Impact: A bad runtime directory or attacker-planted path can cause token truncation/following, and crashes can leave a torn token. This weakens the credential used to authorize web management mutations.

Remediation: Validate runtime directory ownership, type, and mode before use. Write the token via a `0600` temp file opened without following symlinks, `sync_all`, atomic rename, parent directory sync, and final metadata verification.

### HIGH-05: Unix IPC socket binding does not fail closed on unsafe runtime directories
Severity: High

Evidence: `crates/pcloud-ipc/src/transport.rs:711` creates or chmods the parent directory only if ownership matches, but proceeds otherwise. Existing socket paths are blindly removed at `crates/pcloud-ipc/src/transport.rs:726`, and `Drop` removes `self.socket_path` without inode/type validation at `crates/pcloud-ipc/src/transport.rs:624`. Daemon bootstrap similarly provisions dirs with create/chmod only at `crates/pcloud-daemon/src/bootstrap.rs:441`.

Impact: Misconfigured or attacker-controlled runtime directories can lead to unsafe socket placement or path removal races. This violates enterprise runtime-directory hygiene requirements.

Remediation: Centralize managed-dir validation using `symlink_metadata`: reject symlinks, require directory, current UID ownership, and no group/other access. Before unlinking, require an existing entry to be a socket owned by the daemon user; on drop, verify the path still refers to the bound socket.

### HIGH-06: Windows IPC slow-client isolation is explicitly a no-op
Severity: High

Evidence: The transport docs state Windows read/write timeouts are no-ops at `crates/pcloud-ipc/src/transport.rs:24`. The server sets read timeouts at `crates/pcloud-ipc/src/transport.rs:836`, but `WindowsStream::set_read_timeout` just returns `Ok(())` at `crates/pcloud-ipc/src/platform/windows.rs:667`. Reads are blocking `ReadFile` loops at `crates/pcloud-ipc/src/platform/windows.rs:584`.

Impact: A same-user Windows client can hold named-pipe worker threads indefinitely with partial frames until per-peer/global connection caps are exhausted.

Remediation: Implement overlapped named-pipe I/O with per-read/write deadlines and cancellation, or move Windows pipe handling to async I/O with enforced timeouts. Add Windows tests for slow header/body clients and subsequent request acceptance.

### HIGH-07: Windows service wrapper reports daemon failures as clean stops
Severity: High

Evidence: `service_main` swallows `run_service` errors at `crates/pcloud-daemon-win/src/main.rs:159`. Worker bootstrap/serve errors and panics are swallowed at `crates/pcloud-daemon-win/src/main.rs:246`, and the service reports `Stopped` with `Win32(0)` at `crates/pcloud-daemon-win/src/main.rs:257`.

Impact: SCM cannot distinguish operator stop from daemon startup failure, fatal serve error, or panic. Restart-on-failure policy and monitoring may not trigger.

Remediation: Preserve worker result and requested-stop state. Report non-zero service exit codes for bootstrap errors, serve errors, and panics; only report zero for requested graceful stop. Log failures to Windows Event Log or stderr.

### MEDIUM-08: IPC decoders accept frames tagged as the wrong message kind
Severity: Medium

Evidence: `decode_request` reads `message_type` but still deserializes request payloads tagged as response/event at `crates/pcloud-ipc/src/protocol.rs:268`. `decode_response` does the symmetric behavior at `crates/pcloud-ipc/src/protocol.rs:321`.

Impact: Wire-level request/response/event separation is not enforced, creating compatibility ambiguity and future event-dispatch risk.

Remediation: Reject unexpected message kinds before payload deserialization with a typed protocol error. Add tests for valid request JSON tagged as response/event and valid response JSON tagged as request/event.

### MEDIUM-09: IPC clients allocate response payloads before enforcing protocol caps
Severity: Medium

Evidence: Unix clients use `read_to_end` before parsing/cap checks at `crates/pcloud-ipc/src/transport.rs:790`. Windows clients read the payload length and allocate `vec![0u8; payload_len]` before checking caps at `crates/pcloud-ipc/src/transport.rs:804`.

Impact: A compromised or spoofed daemon endpoint can force CLI/SDK clients into excessive allocation or OOM before protocol validation rejects the frame.

Remediation: Use a shared framed-response reader that reads the header, validates `payload_len <= MAX_IPC_PAYLOAD_LEN`, then allocates and reads exactly that payload. Add malicious-server tests for oversized response lengths.

### MEDIUM-10: TFA/recovery code IPC value is a plain `String` with derived `Debug`
Severity: Medium

Evidence: `Request` derives `Debug` at `crates/pcloud-ipc/src/methods.rs:260`. `TwoFactorCodeSubmission.value` is a plain `String` at `crates/pcloud-ipc/src/methods.rs:287`; it is wrapped in `SecretString` only later in daemon runtime at `crates/pcloud-daemon/src/runtime.rs:545`.

Impact: OTP or recovery-code values can leak through debug formatting, panic output, tests, or future observability changes before being wrapped.

Remediation: Use a redacted serde-transparent secret wrapper for `TwoFactorCodeSubmission.value`, convert to `SecretString` only through an explicit consuming API, and add a regression asserting debug output never contains the submitted code.

### MEDIUM-11: Web CSRF design is incompatible with rendered no-JS forms
Severity: Medium

Evidence: CSRF requires the cookie value in `X-CSRF-Token` at `crates/pcloud-web/src/routes.rs:18`, but CSP disables scripts at `crates/pcloud-web/src/routes.rs:61`, the cookie is `HttpOnly` at `crates/pcloud-web/src/routes.rs:761`, and the rendered forms at `crates/pcloud-web/src/routes.rs:815` do not include hidden CSRF fields.

Impact: Browser users cannot submit the provided management forms successfully without manual header tooling, degrading operational usability and encouraging unsafe workarounds.

Remediation: For no-JS forms, include a hidden CSRF field and validate it against the cookie or server-side token store. Keep the web-token authorization check separate.

### MEDIUM-12: Web non-loopback bind validation panics instead of returning a config error
Severity: Medium

Evidence: Non-loopback bind uses `panic!` at `crates/pcloud-web/src/lib.rs:237`; `serve` asserts loopback at `crates/pcloud-web/src/lib.rs:372`; `bind_for_test` asserts at `crates/pcloud-web/src/lib.rs:443`.

Impact: Bad config or environment can crash the daemon/web wrapper and cause restart loops instead of a clear operator-facing configuration failure.

Remediation: Replace panics/asserts with a typed `WebError::NonLoopbackBind { addr }`, surface it through daemon/service startup, and keep tests asserting error return rather than panic.

### MEDIUM-13: Mount crash-recovery refusal is logged but not enforced
Severity: Medium

Evidence: `check_orphans` can return rejected orphan paths at `crates/pcloud-daemon/src/mount_runtime.rs:413`. Bootstrap logs "refusing to start mount service" at `crates/pcloud-daemon/src/bootstrap.rs:802`, but still returns a live `MountControl` in the runtime shell at `crates/pcloud-daemon/src/bootstrap.rs:830`.

Impact: Operators receive a false safety signal; later mount operations can still proceed unless another conflict catches them.

Remediation: Convert rejected orphan scans into a bootstrap error, or mark mount control disabled with the rejected paths so mount/status operations return an explicit conflict until resolved.

### LOW-14: Graceful mount shutdown leaves stale `mount_pid` sidecar on drop path
Severity: Low

Evidence: Explicit unmount removes the pidfile at `crates/pcloud-daemon/src/mount_runtime.rs:584`, but `MountControl::Drop` only calls ordered shutdown at `crates/pcloud-daemon/src/mount_runtime.rs:760` and does not remove the sidecar written at `crates/pcloud-daemon/src/mount_runtime.rs:348`.

Impact: A clean SIGTERM can look like a crashed daemon on next boot, adding stale-pid recovery noise and weakening crash-recovery signal quality.

Remediation: Remove `mount_pid` after successful ordered shutdown if this daemon wrote it, and retain it only when shutdown fails and the mount still appears live.

### LOW-15: Soft invalid TFA code leaves session state inconsistent with the transition table
Severity: Low

Evidence: The transition table documents `MarkTwoFactorCodeInvalid` returning to `TwoFactorRequired` at `crates/pcloud-auth/src/manager.rs:63`, but the reducer only sets `last_auth_error` at `crates/pcloud-auth/src/manager.rs:319`. The TFA submit path sets `AuthenticatingWithPassword` at `crates/pcloud-auth/src/manager.rs:277`, and soft TFA failure applies `MarkTwoFactorCodeInvalid` at `crates/pcloud-auth/src/orchestrator.rs:383`.

Impact: Status consumers can observe an "authenticating" state after a wrong TFA code instead of a promptable TFA-required state.

Remediation: Set `snapshot.state = SessionState::TwoFactorRequired` when applying `MarkTwoFactorCodeInvalid`, and add a reducer/orchestrator regression test.

## Commands Run

- `sed -n '1,240p' pcloud_rev.md`
- `rg --files crates/pcloud-ipc crates/pcloud-daemon crates/pcloud-web crates/pcloud-config crates/pcloud-session crates/pcloud-auth crates/pcloud-daemon-win | sort`
- `find crates/pcloud-ipc crates/pcloud-daemon crates/pcloud-web crates/pcloud-config crates/pcloud-session crates/pcloud-auth crates/pcloud-daemon-win -maxdepth 3 -type f`
- `nl -ba` on audited source files in the scoped crates, including IPC protocol/transport/platform files, daemon runtime/serve/bootstrap/mount files, web lib/routes/templates/tests, config API/runtime/env/path files, auth/session lifecycle files, and Windows service wrapper.
- `rg -n` for privileged IPC methods, API-server mutation, shutdown, web-token handling, CSRF/Host/Origin/CORS, runtime-dir handling, request size/timeouts, serialization, and crash-recovery paths.

## Limitations

No build or test commands were run because this subagent was instructed not to modify files, and normal Rust builds/tests would write `target/`. Windows behavior was reviewed source-only from Linux; no Windows SCM or named-pipe integration tests were executed. No live pCloud account, mounted filesystem, or running daemon was exercised.

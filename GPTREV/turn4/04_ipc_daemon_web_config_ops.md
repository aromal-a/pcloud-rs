# pcloud-rs Turn 4 Audit: IPC / Daemon / Web / Config / Ops

Read-only audit using `pcloud_rev.md` as the master prompt. No files were edited.

## Findings

### H-01 Daemon Config Files Are Not Loaded

Severity: High

Evidence: `bootstrap_shell()` constructs secure defaults from `PCLOUD_ROOT` or discovered directories at `crates/pcloud-daemon/src/bootstrap.rs:377`, applies env overrides at `crates/pcloud-daemon/src/bootstrap.rs:391`, and calls `bootstrap_with_config()` at `crates/pcloud-daemon/src/bootstrap.rs:392`. It never reads `PCLOUD_CONFIG` or calls the strict loader in `crates/pcloud-config/src/loader.rs:126`. The runtime sets `config_path: None` at `crates/pcloud-daemon/src/bootstrap.rs:872`, so SIGHUP reload only runs when `runtime.config_path` is `Some` at `crates/pcloud-daemon/src/serve.rs:434`. macOS packaging sets `PCLOUD_CONFIG` at `packaging/macos/com.pcloud.pcloud-rs.plist:116`, but daemon code does not consume it.

Impact: packaged/operator config files do not affect daemon startup. File permission/schema validation exists but is bypassed. SIGHUP reload is effectively dead in normal daemon startup.

Remediation: add explicit config resolution for `--config`, `PCLOUD_CONFIG`, and platform defaults. Load with `ConfigProfile::load_with_validation()`, then apply env overrides, and store the resolved path in `RuntimeShell.config_path`. Add an integration test proving SIGHUP reload applies a changed config file.

### H-02 systemd Unit Uses `Type=notify`, But `pcloudd serve` Does Not Send `READY=1`

Severity: High

Evidence: `packaging/systemd/pcloudd.service:31` sets `Type=notify`, `packaging/systemd/pcloudd.service:36` sets `NotifyAccess=main`, and `packaging/systemd/pcloudd.service:37` enables `WatchdogSec=30s`. The Unix `pcloudd serve` path binds and prints daemon listening at `crates/pcloud-daemon/src/main.rs:122-153`, then enters `serve_until_shutdown()` at `crates/pcloud-daemon/src/main.rs:235`. `READY=1` is only sent in `serve_with_shutdown()` at `crates/pcloud-daemon/src/serve.rs:590`, which is the Windows/embedder path, not the Unix CLI path. The normal serve loop emits `WATCHDOG=1` at `crates/pcloud-daemon/src/serve.rs:487`.

Impact: a packaged systemd service can remain activating and time out even after the daemon has bound IPC.

Remediation: send `sd_notify("READY=1\n")` in the Unix `pcloudd serve` path after successful bind and background-worker startup, or change the unit to `Type=simple`. Add a fake `NOTIFY_SOCKET` integration test.

### H-03 systemd Packaging Does Not Anchor Daemon Paths To Managed Directories

Severity: High

Evidence: the service creates `RuntimeDirectory=pcloud-rs`, `StateDirectory=pcloud-rs`, and `LogsDirectory=pcloud-rs` at `packaging/systemd/pcloudd.service:73`, `packaging/systemd/pcloudd.service:75`, and `packaging/systemd/pcloudd.service:77`, but `PCLOUD_ROOT=%S/pcloud-rs` is commented out at `packaging/systemd/pcloudd.service:153`. Without `PCLOUD_ROOT`, daemon discovery uses `PcloudDirs::discover()` at `crates/pcloud-daemon/src/bootstrap.rs:382`, which uses ProjectDirs and a runtime fallback at `crates/pcloud-config/src/paths.rs:220-238`. The socket unit listens on `%t/pcloud-rs/daemon.sock` at `packaging/systemd/pcloudd.socket:8`, while the daemon default socket filename is `pcloud.sock` at `crates/pcloud-config/src/paths.rs:92`.

Impact: under `DynamicUser=yes`, `ProtectSystem=strict`, and `ProtectHome=tmpfs`, the daemon may fail to find a usable home or write outside the directories systemd prepared. The optional socket unit does not point at the daemon's actual socket.

Remediation: wire `PCLOUD_ROOT` or direct `STATE_DIRECTORY`/`RUNTIME_DIRECTORY` support into startup. Align `pcloudd.socket` with `paths.ipc_socket_path()` or remove socket activation until `LISTEN_FDS` is implemented.

### H-04 Windows Packaged Service Is Unreachable By Normal User IPC Clients

Severity: High

Evidence: the WiX service installs as `NT SERVICE\pcloudd` at `packaging/windows/wix/pcloud-rs.wxs:84-91`. Windows IPC server pipe names are derived from the service process SID at `crates/pcloud-ipc/src/platform/windows.rs:286-291`. Client pipe names are derived from the client process SID at `crates/pcloud-ipc/src/platform/windows.rs:695-701`. The pipe DACL allows only the owner SID at `crates/pcloud-ipc/src/platform/windows.rs:758-761`, and peer SID mismatch is rejected at `crates/pcloud-ipc/src/platform/windows.rs:496`.

Impact: `pcloudc.exe` run by an interactive user looks for a different pipe than the installed service creates, and the DACL would reject it anyway.

Remediation: choose one Windows model: per-user agent/service running as the same user, or machine service with an explicit broker ACL and per-request authorization. Add a Windows install smoke test for `pcloudc status` against the service.

### H-05 Windows Service Reports Failures As Clean Stops

Severity: High

Evidence: `service_main()` swallows `run_service()` errors at `crates/pcloud-daemon-win/src/main.rs:159-164`. Worker errors and panics are discarded at `crates/pcloud-daemon-win/src/main.rs:246-254`. Final SCM status always uses `ServiceExitCode::Win32(0)` at `crates/pcloud-daemon-win/src/main.rs:257-265`.

Impact: SCM recovery, monitoring, and operators cannot distinguish bootstrap failure, worker panic, and intentional stop.

Remediation: preserve the worker result, log it to an operational sink, and report nonzero `ServiceExitCode` on error or panic. Add tests for bootstrap failure and worker panic paths.

### H-06 Web Management Read Routes Expose Daemon State Without Token Auth

Severity: High

Evidence: the web crate states it relies on same-user IPC and has no per-request auth at `crates/pcloud-web/src/lib.rs:49-62`. `GET /sync` calls `GetSyncRoots` and `GetPending` without `require_web_token()` at `crates/pcloud-web/src/routes.rs:165-187`. `GET /publinks` calls `ListPublicLinks` without token auth at `crates/pcloud-web/src/routes.rs:252-265`. `GET /activity` exposes notification/activity data without token auth at `crates/pcloud-web/src/routes.rs:401-429`. `GET /settings` exposes the daemon socket path at `crates/pcloud-web/src/routes.rs:436-441`.

Impact: loopback TCP does not enforce same UID. On multi-user hosts, any local user/process can read sync roots, public-link metadata, activity, and settings through the web surface.

Remediation: require `X-PCloud-Web-Token` or a real local session for every daemon-backed route except liveness probes. Add Host/Origin checks and tests proving unauthenticated reads are rejected.

### M-01 `SetApiServer` Persists Rejected API Hints And Reports Success

Severity: Medium

Evidence: `apply_api_server_hint()` rejects non-pCloud domains at `crates/pcloud-config/src/api.rs:292-300`. `RuntimeShell::set_api_server()` logs that error but continues at `crates/pcloud-daemon/src/runtime.rs:2887-2893`, persists the rejected value at `crates/pcloud-daemon/src/runtime.rs:2900-2902`, and the IPC handler returns audited success at `crates/pcloud-daemon/src/runtime.rs:3474-3479`.

Impact: invalid API-server state becomes durable and clients are told the operation succeeded.

Remediation: treat config-level hint rejection as fatal. Return `InvalidRequest`, do not persist the value, and add a regression test for `evil.example.com`.

### M-02 Web Token File Creation Is Not Fail-Closed

Severity: Medium

Evidence: token directory creation is at `crates/pcloud-web/src/lib.rs:316-317`. Directory chmod is best-effort and ignored at `crates/pcloud-web/src/lib.rs:323-327`. The token file is opened with `create(true).truncate(true)` at `crates/pcloud-web/src/lib.rs:329-334`, with no `create_new`, `O_NOFOLLOW`, owner/mode validation, temp-file rename, or `sync_all()`.

Impact: a pre-existing weak directory, file, or symlink in a misconfigured runtime dir can expose the token or clobber an unexpected target.

Remediation: validate parent ownership/mode with `symlink_metadata`, reject symlinks, write a new `0600` temp file with no-follow semantics, `sync_all()`, atomically rename, and verify final metadata.

### M-03 Web HTML Forms Cannot Submit Successfully As Rendered

Severity: Medium

Evidence: mutations require `X-CSRF-Token` at `crates/pcloud-web/src/routes.rs:671-697` and `X-PCloud-Web-Token` at `crates/pcloud-web/src/routes.rs:719-739`. The CSRF cookie is `HttpOnly` at `crates/pcloud-web/src/routes.rs:761`, CSP disables scripts at `crates/pcloud-web/src/routes.rs:61`, and rendered forms contain no hidden CSRF or web-token fields at `crates/pcloud-web/src/routes.rs:815-827` and `crates/pcloud-web/src/routes.rs:843-848`.

Impact: browser users cannot perform legitimate mutations through the rendered UI; only custom clients that add headers can.

Remediation: implement a real session flow with hidden CSRF fields, or intentionally expose a JS-readable double-submit token and adjust CSP. Add UI tests for successful form POSTs.

### M-04 IPC Decoders Accept Wrong Message Kinds

Severity: Medium

Evidence: `decode_request()` deserializes a request payload before mapping `message_type` at `crates/pcloud-ipc/src/protocol.rs:288-298`. It maps `2` to `Response` and unknown values to `Event` but still returns `Ok`. `decode_response()` has the same pattern at `crates/pcloud-ipc/src/protocol.rs:338-348`.

Impact: semantically invalid frames can pass decoding if the payload shape overlaps, weakening protocol compatibility and fuzz boundaries.

Remediation: reject unexpected message kinds before deserialization with a typed protocol error. Add tests for request-as-response and response-as-request frames.

### M-05 IPC Clients Read Or Allocate Unbounded Response Payloads Before Enforcing The Cap

Severity: Medium

Evidence: Unix client IPC reads the entire response with `read_to_end()` at `crates/pcloud-ipc/src/transport.rs:790-792`. Windows client reads `payload_len` from the header and allocates `vec![0u8; payload_len]` before checking the protocol cap at `crates/pcloud-ipc/src/transport.rs:804-816`. The 1 MiB cap is enforced later by `decode_response()` at `crates/pcloud-ipc/src/protocol.rs:338-340`.

Impact: a malicious or stale IPC endpoint can force client memory growth before validation.

Remediation: use a framed reader for responses: read the 8-byte header, validate version/type/length and cap, then allocate and read the payload.

### M-06 IPC Runtime Directory Binding Does Not Fully Validate Existing Parents

Severity: Medium

Evidence: existing socket parents are chmodded only if `metadata(parent).uid() == owner_uid` at `crates/pcloud-ipc/src/transport.rs:717-722`. Bind proceeds otherwise. Existing socket paths are removed at `crates/pcloud-ipc/src/transport.rs:726-727` without symlink/non-directory/final-owner validation.

Impact: misowned or symlinked runtime paths can cause socket squatting, denial of service, or unexpected path use in packaged deployments.

Remediation: validate each parent with `symlink_metadata`, reject symlinks/non-directories/misowned dirs, require mode no broader than `0700`, and verify socket mode after bind.

### M-07 macOS Plists Override The Tested Binary API Default

Severity: Medium

Evidence: secure defaults use `bineapi.pcloud.com:443` at `crates/pcloud-config/src/api.rs:191-214`. The user LaunchAgent overrides host and SNI to `api.pcloud.com` at `packaging/macos/com.pcloud.pcloud-rs.plist:112-115`. The system LaunchDaemon does the same at `packaging/macos/com.pcloud.pcloudd.plist:109-112`.

Impact: macOS packaged runtime can use an endpoint different from the binary API default tested by config and protocol code.

Remediation: remove these overrides or set both host and server name to `bineapi.pcloud.com` unless a separately tested REST transport mode is selected.

### M-08 macOS System LaunchDaemon Conflicts With Same-UID IPC Model

Severity: Medium

Evidence: the LaunchDaemon runs as `_pcloudd` at `packaging/macos/com.pcloud.pcloudd.plist:83-86` and uses `/var/lib/pcloudd` at `packaging/macos/com.pcloud.pcloudd.plist:97-104`. IPC authorization is owner-only at `crates/pcloud-ipc/src/server.rs:121-132`, and the Unix socket is chmodded `0600` at `crates/pcloud-ipc/src/transport.rs:730-731`.

Impact: a normal user CLI or web process cannot control the daemon, and user-scoped auth/keychain/FUSE expectations do not match a system daemon.

Remediation: make the per-user LaunchAgent the default. Treat LaunchDaemon as a separate enterprise/headless deployment that needs an explicit admin broker/auth model.

### M-09 Metrics Exporter Has Unbounded Connection Threads And Ignores The Config Flag

Severity: Medium

Evidence: the exporter spawns one thread per accepted connection at `crates/pcloud-observability/src/exporter.rs:218-225`, with no in-flight cap. `metrics_enabled` defaults false at `crates/pcloud-config/src/observability.rs:68-73`, but the feature-gated daemon path calls `spawn_from_env()` directly at `crates/pcloud-daemon/src/main.rs:210-214`. `spawn_from_env()` reads only environment-derived exporter config at `crates/pcloud-daemon/src/metrics_server.rs:183-190`.

Impact: a loopback scrape flood can create unbounded threads, and operator config does not actually gate the listener when the feature is compiled.

Remediation: add a connection cap like the health server, and gate spawn on `config.observability.metrics_enabled` or remove/rename that flag.

### M-10 Env-Mutating Config Tests Race Under Cargo Parallelism

Severity: Medium

Evidence: `env_pcloud_api_host_overrides_host()` mutates `PCLOUD_API_HOST` and assumes a single-threaded test binary at `crates/pcloud-config/tests/config_validation.rs:111-121`. `env_invalid_port_returns_typed_error()` mutates `PCLOUD_API_PORT` at `crates/pcloud-config/tests/config_validation.rs:203-209`.

Impact: default parallel Cargo test execution can leak env state across tests. During audit, the host override test observed `PCLOUD_API_PORT=not-a-port` and failed.

Remediation: serialize env-mutating tests with `serial_test` or a global mutex, or use a scoped temp-env helper.

## Commands And Results

- `systemd-analyze verify packaging/systemd/pcloudd.service packaging/systemd/pcloudd.socket`: failed because `/usr/bin/pcloudd` does not exist in this workspace.
- `cargo test -p pcloud-ipc --tests`: passed, 64 non-ignored tests.
- `cargo test -p pcloud-config --tests`: failed; unit tests passed, but `env_pcloud_api_host_overrides_host` failed from parallel env mutation.
- `cargo test -p pcloud-web --tests`: passed, 19 tests.

# Turn 5 Review: IPC / Daemon / Web / Config / Ops

Read-only review of the dirty working tree after Turn 4. The review focused on remaining or newly introduced issues; Turn 4 fixes that appear addressed are not repeated except where they expose a new failure mode.

## Findings

### H-01 Privileged IPC Requests Are Audited But Not Authorized

Severity: High

Evidence: `crates/pcloud-daemon/src/serve.rs:101` classifies shutdown, crypto reset, auth persistence, sync removal, backup/device, and public-link operations as privileged. `crates/pcloud-daemon/src/serve.rs:249` only logs those requests, then `serve.rs:269` dispatches them. The IPC server authorizes only exact owner UID at `crates/pcloud-ipc/src/server.rs:121`. The dispatch layer only rate-limits at `crates/pcloud-daemon/src/dispatch.rs:391`. Runtime then executes sensitive handlers such as crypto reset and shutdown at `crates/pcloud-daemon/src/runtime.rs:485` and `:491`, and backup deletion at `runtime.rs:898`.

Impact: any same-UID local process that can reach the socket can perform destructive or credential-sensitive daemon operations. Same UID is not an enterprise authorization boundary.

Remediation: add per-request capability checks before dispatch. Default-deny privileged methods unless the caller has an admin token, explicit local approval, or an allow-listed capability. Add negative IPC tests for `Shutdown`, `CryptoReset`, `DeleteBackup`, and public-link creation without capability.

### H-02 systemd User Unit Path Still Refuses To Load

Severity: High

Evidence: the service file advertises direct user-scope usage at `packaging/systemd/pcloudd.service:7`, but also ships `DynamicUser=yes` at `pcloudd.service:48`. `systemd-analyze --user verify packaging/systemd/pcloudd.service packaging/systemd/pcloudd.socket` fails with `DynamicUser= enabled for user unit, which is not supported. Refusing.` The socket then cannot start because the service is not loaded.

Impact: a documented/user-facing install path can fail before daemon start. This also hides downstream IPC behavior from validation.

Remediation: split system and user units, or make the shipped unit unambiguously system-only and ship a separate user unit without `DynamicUser=`, `PrivateUsers=`, and system-only directory assumptions. Add `systemd-analyze --user verify` and system-unit verify jobs to CI.

### H-03 systemd Watchdog Can Kill A Healthy Idle Daemon

Severity: High

Evidence: the unit enables `WatchdogSec=30s` at `packaging/systemd/pcloudd.service:35`. The normal serve loop sets accept timeout from session refresh at `crates/pcloud-daemon/src/serve.rs:289`, capped to 60 seconds at `crates/pcloud-daemon/src/session_refresh.rs:70`, with a default refresh interval of 300 seconds at `crates/pcloud-config/src/auth.rs:139`. `WATCHDOG=1` is emitted only after the accept loop wakes at `serve.rs:488`. In metrics builds, `crates/pcloud-daemon/src/metrics_server.rs:121` has its own loop with no watchdog notify.

Impact: an idle daemon can miss the 30 second watchdog deadline and be restarted by systemd despite being healthy.

Remediation: derive the accept timeout from `$WATCHDOG_USEC` and send heartbeats at no more than half the watchdog interval, including the metrics-enabled loop. Alternatively remove `WatchdogSec` until this is guaranteed. Add an integration test with a fake `NOTIFY_SOCKET`.

### H-04 macOS LaunchAgent `PCLOUD_CONFIG` Now Breaks First Launch

Severity: High

Evidence: the user LaunchAgent sets `PCLOUD_CONFIG={{USER_HOME}}/.config/pcloud-rs/config.toml` at `packaging/macos/com.pcloud.pcloud-rs.plist:116`. The installer creates several user directories at `packaging/macos/install.sh:63`, but not that config file. The daemon now treats any `PCLOUD_CONFIG` value as mandatory at `crates/pcloud-daemon/src/bootstrap.rs:391`, and the loader reads JSON, not TOML, at `crates/pcloud-config/src/loader.rs:134`. The config reference confirms JSON at `docs/book/src/reference/config.md:25`.

Impact: Turn 4's config-loading fix turns the existing macOS plist into a startup failure on a clean install, or a parse failure if the operator follows the TOML path.

Remediation: remove `PCLOUD_CONFIG` from the LaunchAgent unless the installer writes a valid `0600` JSON config. If a config path is required, use `config.json`, create the parent as `0700`, and add a launchd-template bootstrap smoke test.

### H-05 Windows MSI Service IPC Is Unreachable From Normal User Clients

Severity: High

Evidence: the MSI installs the daemon as `NT SERVICE\pcloudd` at `packaging/windows/wix/pcloud-rs.wxs:84`. The server pipe name is derived from the service process SID at `crates/pcloud-ipc/src/platform/windows.rs:290`. The client derives a different pipe name from the interactive user SID at `windows.rs:708`. Even if paths matched, the pipe DACL only allows the owner SID at `windows.rs:771`, and peer SID mismatch is rejected at `windows.rs:507`.

Impact: a normal `pcloudc.exe` user cannot control or query the installed service.

Remediation: choose either a per-user agent model or a machine-service model with a stable pipe name, explicit DACL for authorized users/groups, and per-request capability checks. Add an MSI smoke test proving `pcloudc status` works after install.

### H-06 Windows Service Reports Failures As Clean Stops

Severity: High

Evidence: `service_main` swallows `run_service()` errors at `crates/pcloud-daemon-win/src/main.rs:159`. Worker errors and panics are discarded at `main.rs:246`. The service always reports `ServiceExitCode::Win32(0)` at `main.rs:257`.

Impact: bootstrap failures and daemon panics are invisible to SCM recovery policy and operators see a clean stop.

Remediation: propagate worker result to SCM status, report nonzero service-specific or Win32 exit codes, and write Windows Event Log entries. Add tests/mocks for bootstrap failure and worker panic.

### H-07 Web UI Is Documented As Runnable But Has No Binary Target

Severity: High

Evidence: `crates/pcloud-web/Cargo.toml:1` defines only the package and dependencies, with no binary target. `cargo metadata` shows `pcloud_web` lib plus tests only. Yet README says `cargo run -p pcloud-web` at `README.md:122`, and the web UI runbook says the same at `docs/book/src/operations/web-ui.md:78`. The command fails with `error: a bin target must be available for cargo run`.

Impact: the web management surface cannot be started as documented or packaged as `pcloud-web`.

Remediation: add a `src/main.rs` binary that parses `--bind`, `--socket`, token options, and ready signaling, then calls `pcloud_web::serve`. Add `cargo run -p pcloud-web -- --help` to CI.

### M-01 Metrics Feature Path Ignores Lifecycle And Config Controls

Severity: Medium

Evidence: `metrics_enabled` defaults false at `crates/pcloud-config/src/observability.rs:68`, but metrics builds unconditionally call `spawn_from_env` at `crates/pcloud-daemon/src/main.rs:210`. The metrics loop consumes SIGHUP reload requests as a no-op at `crates/pcloud-daemon/src/metrics_server.rs:140`, while the normal loop applies reloads at `crates/pcloud-daemon/src/serve.rs:436`. The exporter spawns one thread per connection at `crates/pcloud-observability/src/exporter.rs:218`.

Impact: operators can believe metrics are disabled when a metrics-feature binary still opens the exporter, SIGHUP reload behavior changes by build feature, and connection floods can consume threads.

Remediation: gate exporter startup on `config.observability.metrics_enabled`, share reload handling between serve loops, and cap in-flight metrics connections with a semaphore or thread pool.

### M-02 macOS Plists Still Override The Binary API Default

Severity: Medium

Evidence: secure API defaults use `bineapi.pcloud.com` at `crates/pcloud-config/src/api.rs:191`. The user LaunchAgent overrides host and SNI to `api.pcloud.com` at `packaging/macos/com.pcloud.pcloud-rs.plist:112`. The system LaunchDaemon does the same at `packaging/macos/com.pcloud.pcloudd.plist:109`.

Impact: macOS packaged runtime uses a different endpoint from the binary-protocol default and likely from the tested path.

Remediation: remove the overrides or set both values to `bineapi.pcloud.com`. If REST mode is intended, model that explicitly and test it.

### M-03 macOS System LaunchDaemon Conflicts With Same-UID IPC

Severity: Medium

Evidence: the LaunchDaemon runs as `_pcloudd` at `packaging/macos/com.pcloud.pcloudd.plist:83` with state under `/var/lib/pcloudd` at `com.pcloud.pcloudd.plist:97`. IPC accepts only the owner UID at `crates/pcloud-ipc/src/server.rs:121`, and Unix sockets are mode `0600` at `crates/pcloud-ipc/src/transport.rs:737`.

Impact: interactive users cannot control a daemon running as `_pcloudd` without a separate broker or ACL model.

Remediation: make LaunchAgent the supported interactive path. Mark LaunchDaemon headless/experimental, or add an admin broker with explicit capability authorization.

### M-04 Web Forms Still Cannot Submit As Rendered

Severity: Medium

Evidence: CSP disables scripts at `crates/pcloud-web/src/routes.rs:63`. Mutations require an `X-CSRF-Token` header at `routes.rs:688`, and daemon-backed routes require `X-PCloud-Web-Token` at `routes.rs:736`. The CSRF cookie is `HttpOnly` at `routes.rs:778`. Rendered forms at `routes.rs:832` and `routes.rs:860` cannot set either header.

Impact: the web UI renders mutation forms that a browser cannot successfully submit.

Remediation: either make the UI explicitly read-only, or implement a real session cookie plus hidden CSRF form field accepted by the server. Add browser-like POST tests without custom headers.

### L-01 Web Surface Has No Host Or Origin Enforcement

Severity: Low

Evidence: the router attaches management routes directly at `crates/pcloud-web/src/routes.rs:74`. Token and CSRF checks at `routes.rs:688` and `routes.rs:736` do not validate `Host`, `Origin`, or `Referer`. `rg` found no Host/Origin enforcement in `crates/pcloud-web/src/routes.rs`.

Impact: the token-header design reduces browser CSRF risk, but DNS rebinding and reverse-proxy misrouting are still not explicitly rejected.

Remediation: reject non-loopback or non-configured `Host`, and require same-origin `Origin` or `Referer` on mutating routes. Add tests for malicious Host and cross-origin mutation attempts.

## Commands / Results

| Command | Result |
|---|---|
| `sed -n '1,240p' pcloud_rev.md` | Read master review prompt. |
| `git status --short` | Dirty tree with many modified and untracked review/fix artifacts; no files edited during this review. |
| `cargo test -p pcloud-ipc --tests` | Passed: 67 tests passed, 1 ignored. |
| `cargo test -p pcloud-web --tests` | Passed: 24 tests passed. |
| `cargo test -p pcloud-config --tests` | Passed: 132 tests passed. |
| `cargo check -p pcloud-daemon --features metrics` | Passed; emitted existing vendored password dictionary warning from `pcloud-crypto`. |
| `cargo run -p pcloud-web -- --help` | Failed as expected: `a bin target must be available for cargo run`. |
| `systemd-analyze --user verify packaging/systemd/pcloudd.service packaging/systemd/pcloudd.socket` | Failed: `DynamicUser= enabled for user unit, which is not supported`; socket could not start because service was not loaded. |
| `systemd-analyze verify packaging/systemd/pcloudd.service packaging/systemd/pcloudd.socket` | Failed in this environment because `/usr/bin/pcloudd` is not installed. |
| `plutil -lint packaging/macos/*.plist` | Not available in this Linux environment: `plutil: command not found`. |
| `rg` and `nl -ba` inspections across IPC, daemon, web, config, systemd, launchd, Windows service files | Static evidence cited above. |

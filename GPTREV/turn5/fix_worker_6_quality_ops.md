# Turn 5 Fix Worker 6: Code Quality / Ops

Date: 2026-04-30

## Scope

Inputs:
- `GPTREV/turn5/06_code_quality_dependency_inventory.md`
- `GPTREV/turn5/04_ipc_daemon_web_config_ops.md`

Ownership honored:
- Edited only `crates/pcloud-idp/**`, crate-level MSRV rustdoc comments in `crates/*/src/lib.rs`, `crates/pcloud-daemon-win/**`, `crates/pcloud-daemon/src/serve.rs` for health/systemd/watchdog behavior, `crates/pcloud-daemon/src/metrics_server.rs`, `crates/pcloud-observability/src/exporter.rs`, and this report.
- Did not edit web routes, packaging, sync runtime, proto/ipc schema, or daemon `main.rs`.
- Preserved pre-existing dirty changes in owned files, including the existing `crates/pcloud-daemon-win/Cargo.toml` MSRV metadata change.

## Changes

- Fixed `pcloud-idp` no-default clippy by moving `Duration` and `ExposeSecret` imports into the `oidc-http-exchange`-gated module/test scope where they are used.
- Updated stale crate-level MSRV rustdoc text from Rust 1.82 to Rust 1.85 in:
  - `crates/pcloud-backends/src/lib.rs`
  - `crates/pcloud-daemon/src/lib.rs`
  - `crates/pcloud-fs/src/lib.rs`
  - `crates/pcloud-ipc/src/lib.rs`
  - `crates/pcloud-proto/src/lib.rs`
  - `crates/pcloud-session/src/lib.rs`
- Improved Windows service failure reporting:
  - worker `anyhow::Error` exits now report `ServiceExitCode::ServiceSpecific(1)`;
  - worker panics report `ServiceExitCode::ServiceSpecific(2)`;
  - pre-final-SCM lifecycle errors write a failure message and exit the process non-zero;
  - failure details are mirrored through `eventcreate.exe` with stderr fallback instead of being discarded.
- Improved daemon health/systemd ops handling in `serve.rs`:
  - invalid `PCLOUD_HEALTH_PORT` now fails startup with an explicit error instead of silently disabling health;
  - IPC accept timeout is derived as the minimum of the existing refresh timeout and half of `$WATCHDOG_USEC`;
  - `$WATCHDOG_PID` is respected when deriving watchdog cadence;
  - systemd notify calls are centralized for READY/RELOADING/STOPPING/WATCHDOG.
- Improved metrics-enabled serve loop lifecycle in `metrics_server.rs`:
  - applies watchdog-derived IPC accept timeout;
  - emits watchdog heartbeats after accept-loop iterations;
  - handles accept timeouts instead of treating them as fatal;
  - mirrors config reload handling from the normal serve loop;
  - marks the exporter unhealthy if the bridge snapshot lock is poisoned instead of silently dropping refreshes.
- Hardened metrics exporter in `pcloud-observability`:
  - invalid `PCLOUD_METRICS_PORT` values are reported instead of silently defaulting;
  - in-flight scrape connection handlers are capped at 32;
  - handler slot accounting is released via RAII even if a handler panics;
  - request-line/header read errors no longer use silent `.ok()` conversions.

## Verification

Passed:
- `rustfmt --edition 2024 --check` on all touched Rust files.
- `cargo check -p pcloud-idp --all-targets --no-default-features --locked`
- `cargo clippy -p pcloud-idp --all-targets --no-default-features --locked -- -D warnings`
- `cargo clippy -p pcloud-idp --all-targets --locked -- -D warnings`
- `cargo test -p pcloud-idp --no-default-features --locked`
- `cargo test -p pcloud-idp --locked`
- `cargo check -p pcloud-observability --features prometheus-exporter --locked`
- `cargo clippy -p pcloud-observability --features prometheus-exporter --locked -- -D warnings`
- `cargo test -p pcloud-observability --features prometheus-exporter exporter --locked`
- `cargo check -p pcloud-daemon-win --locked`
- `cargo clippy -p pcloud-daemon-win --locked -- -D warnings`
- `cargo test -p pcloud-daemon-win --locked`

Blocked / failed outside worker ownership:
- `cargo fmt --all --check` fails on formatting in `crates/pcloud-daemon/src/sync_loop_runtime.rs`, which is outside this worker's ownership.
- `cargo check --workspace --all-targets --no-default-features --locked` no longer reaches an idp unused-import failure, but the dirty tree fails in out-of-scope files:
  - `crates/pcloud-web/src/routes.rs` has an `axum::Request` / `pcloud_ipc::Request` import conflict and related type errors;
  - `crates/pcloud-ipc/src/protocol.rs` tests move out of `Request`, which currently implements `Drop`;
  - `crates/pcloud-cli/src/app.rs` and `crates/pcloud-cli/src/commands.rs` tests move out of `Request`, which currently implements `Drop`.
- `cargo check -p pcloud-daemon --features metrics --locked` is blocked by out-of-scope daemon/runtime errors before the touched metrics loop can be fully verified:
  - `crates/pcloud-daemon/src/runtime.rs` has a `SecretString::new(result.auth_token)` type mismatch and many move-out-of-`Request` errors;
  - `crates/pcloud-daemon/src/sync_loop_runtime.rs` references missing helper functions such as `load_sync_root`, `safe_local_target_path`, `io_error_to_pending_fs_error`, and `remote_child_path`.
- `cargo check -p pcloud-daemon-win --target x86_64-pc-windows-gnu --locked` is blocked by missing host toolchain `x86_64-w64-mingw32-gcc` while building `ring`.

## Remaining Notes

- Full config gating of metrics exporter startup on `config.observability.metrics_enabled` requires editing `crates/pcloud-daemon/src/main.rs`, which is outside this worker's ownership. This pass therefore limited metrics work to lifecycle/watchdog/reload behavior in `metrics_server.rs` and connection caps in the exporter.

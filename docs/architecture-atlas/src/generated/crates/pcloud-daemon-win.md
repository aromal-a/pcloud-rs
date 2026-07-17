# `pcloud-daemon-win`

**Maturity:** Experimental / bounded

**Version:** `0.8.1-beta`

**Directory:** `crates/pcloud-daemon-win`

**Manifest:** [`crates/pcloud-daemon-win/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/Cargo.toml)

Experimental unshipped Windows SCM host for the per-user pcloudd runtime.

## Feature-family profile

**Why it exists.** Explore Windows Service Control Manager hosting without contaminating the portable per-user daemon.

**What it is good for.** Experimental Windows service installation and SCM lifecycle integration.

**Why it is good at that job.** The wrapper is isolated and explicitly unshipped, so Windows-specific service semantics cannot be mistaken for the supported daemon contract.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloudd-svc` | bin | [`crates/pcloud-daemon-win/src/main.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs) |

## Direct dependencies

`pcloud-daemon`, `windows-service`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-daemon-win/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-daemon-win/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/README.md) | documentation | pcloud-daemon-win |
| [`crates/pcloud-daemon-win/src/main.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs) | binary root | pcloud-daemon-win |

## Rust declaration index (12 total; 1 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `main` | `private` | fn | [`crates/pcloud-daemon-win/src/main.rs:106`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L106) | Non-Windows entry point: no-op stub. This function is compiled on every target that is **not** Windows. It ex… |
| `svc` | `private` | mod | [`crates/pcloud-daemon-win/src/main.rs:124`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L124) | Windows Service implementation (SCM-hosted). All items in this module are `#\[cfg(windows)\]`-gated and are **n… |
| `SERVICE_NAME` | `private` | const | [`crates/pcloud-daemon-win/src/main.rs:141`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L141) | SCM-visible service name. Must match the name passed to `sc.exe create` at install time. |
| `SERVICE_ERROR_DAEMON_FAILED` | `private` | const | [`crates/pcloud-daemon-win/src/main.rs:142`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L142) | Read the source/rustdoc for the exact contract. |
| `SERVICE_ERROR_WORKER_PANICKED` | `private` | const | [`crates/pcloud-daemon-win/src/main.rs:143`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L143) | Read the source/rustdoc for the exact contract. |
| `SERVICE_TYPE` | `private` | const | [`crates/pcloud-daemon-win/src/main.rs:148`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L148) | Service type reported to the SCM. \[`ServiceType::OWN_PROCESS`\] means the service runs in its own dedicated pr… |
| `service_main` | `private` | fn | [`crates/pcloud-daemon-win/src/main.rs:163`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L163) | SCM entry point. Invoked on the service worker thread spawned by the `windows_service` dispatcher. Returning… |
| `report_service_failure` | `private` | fn | [`crates/pcloud-daemon-win/src/main.rs:172`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L172) | Read the source/rustdoc for the exact contract. |
| `panic_payload_summary` | `private` | fn | [`crates/pcloud-daemon-win/src/main.rs:193`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L193) | Read the source/rustdoc for the exact contract. |
| `run_service` | `private` | fn | [`crates/pcloud-daemon-win/src/main.rs:220`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L220) | Core service lifecycle. 1. Registers an SCM control handler that flips a shared `Arc&lt;AtomicBool&gt;` shutdown fl… |
| `main` | `pub` | fn | [`crates/pcloud-daemon-win/src/main.rs:319`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L319) | Windows-only process entry point. Hands the current process over to the SCM dispatcher via \[`service_dispatch… |
| `main` | `private` | fn | [`crates/pcloud-daemon-win/src/main.rs:331`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-daemon-win/src/main.rs#L331) | Windows entry point. Delegates to \[`svc::main`\] which blocks on the SCM dispatcher. Errors surface the underl… |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

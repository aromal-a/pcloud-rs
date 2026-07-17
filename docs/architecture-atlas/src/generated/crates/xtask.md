# `xtask`

**Maturity:** Repository infrastructure

**Version:** `0.8.1-beta`

**Directory:** `xtask`

**Manifest:** [`xtask/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/Cargo.toml)

Repository-owned local CI/CD orchestrator for pcloud-rs.

## Feature-family profile

**Why it exists.** Keep CI/CD policy versioned and runnable locally instead of depending on opaque hosted-workflow behavior.

**What it is good for.** Format, lint, test, coverage, audit, packaging, Docker, native mount, Windows remote, release, and cleanup orchestration.

**Why it is good at that job.** One Rust entrypoint pins command order, fail/skip policy, toolchain use, and cross-platform evidence so developer and release gates match.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `xtask` | bin | [`xtask/src/main.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs) |

## Direct dependencies

None.

## Cargo features

No declared package features.

## File inventory (2)

| File | Kind | Role |
|---|---|---|
| [`xtask/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`xtask/src/main.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs) | binary root | Repository-owned local CI/CD orchestration. |

## Rust declaration index (41 total; 0 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `TOOLCHAIN` | `private` | const | [`xtask/src/main.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L17) | Read the source/rustdoc for the exact contract. |
| `COVERAGE_FLOOR` | `private` | const | [`xtask/src/main.rs:18`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L18) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_WINDOWS_HOST` | `private` | const | [`xtask/src/main.rs:19`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L19) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_WINDOWS_USER` | `private` | const | [`xtask/src/main.rs:20`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L20) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_WINDOWS_ROOT` | `private` | const | [`xtask/src/main.rs:21`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L21) | Read the source/rustdoc for the exact contract. |
| `TaskResult` | `private` | type | [`xtask/src/main.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L23) | Read the source/rustdoc for the exact contract. |
| `main` | `private` | fn | [`xtask/src/main.rs:25`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L25) | Read the source/rustdoc for the exact contract. |
| `real_main` | `private` | fn | [`xtask/src/main.rs:35`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L35) | Read the source/rustdoc for the exact contract. |
| `print_help` | `private` | fn | [`xtask/src/main.rs:61`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L61) | Read the source/rustdoc for the exact contract. |
| `run_ci` | `private` | fn | [`xtask/src/main.rs:92`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L92) | Read the source/rustdoc for the exact contract. |
| `run_compatibility` | `private` | fn | [`xtask/src/main.rs:113`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L113) | Read the source/rustdoc for the exact contract. |
| `run_release` | `private` | fn | [`xtask/src/main.rs:164`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L164) | Read the source/rustdoc for the exact contract. |
| `run_preflight` | `private` | fn | [`xtask/src/main.rs:174`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L174) | Read the source/rustdoc for the exact contract. |
| `run_host` | `private` | fn | [`xtask/src/main.rs:208`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L208) | Read the source/rustdoc for the exact contract. |
| `run_coverage` | `private` | fn | [`xtask/src/main.rs:282`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L282) | Read the source/rustdoc for the exact contract. |
| `run_packaging` | `private` | fn | [`xtask/src/main.rs:401`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L401) | Read the source/rustdoc for the exact contract. |
| `run_docker` | `private` | fn | [`xtask/src/main.rs:440`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L440) | Read the source/rustdoc for the exact contract. |
| `run_windows` | `private` | fn | [`xtask/src/main.rs:492`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L492) | Read the source/rustdoc for the exact contract. |
| `run_shell_syntax_checks` | `private` | fn | [`xtask/src/main.rs:516`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L516) | Read the source/rustdoc for the exact contract. |
| `ensure_workspace_root` | `private` | fn | [`xtask/src/main.rs:549`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L549) | Read the source/rustdoc for the exact contract. |
| `env_flag` | `private` | fn | [`xtask/src/main.rs:556`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L556) | Read the source/rustdoc for the exact contract. |
| `cargo` | `private` | fn | [`xtask/src/main.rs:565`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L565) | Read the source/rustdoc for the exact contract. |
| `warnings_as_errors` | `private` | fn | [`xtask/src/main.rs:571`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L571) | Read the source/rustdoc for the exact contract. |
| `command` | `private` | fn | [`xtask/src/main.rs:576`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L576) | Read the source/rustdoc for the exact contract. |
| `command0` | `private` | fn | [`xtask/src/main.rs:586`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L586) | Read the source/rustdoc for the exact contract. |
| `step` | `private` | fn | [`xtask/src/main.rs:590`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L590) | Read the source/rustdoc for the exact contract. |
| `output` | `private` | fn | [`xtask/src/main.rs:601`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L601) | Read the source/rustdoc for the exact contract. |
| `require_tool` | `private` | fn | [`xtask/src/main.rs:614`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L614) | Read the source/rustdoc for the exact contract. |
| `tool_available` | `private` | fn | [`xtask/src/main.rs:622`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L622) | Read the source/rustdoc for the exact contract. |
| `Docker` | `private` | struct | [`xtask/src/main.rs:644`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L644) | Read the source/rustdoc for the exact contract. |
| `discover` | `private` | fn | [`xtask/src/main.rs:649`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L649) | Read the source/rustdoc for the exact contract. |
| `run` | `private` | fn | [`xtask/src/main.rs:671`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L671) | Read the source/rustdoc for the exact contract. |
| `WindowsRemote` | `private` | struct | [`xtask/src/main.rs:684`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L684) | Read the source/rustdoc for the exact contract. |
| `from_env` | `private` | fn | [`xtask/src/main.rs:693`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L693) | Read the source/rustdoc for the exact contract. |
| `destination` | `private` | fn | [`xtask/src/main.rs:732`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L732) | Read the source/rustdoc for the exact contract. |
| `ssh_base` | `private` | fn | [`xtask/src/main.rs:736`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L736) | Read the source/rustdoc for the exact contract. |
| `credentialed_ssh_base` | `private` | fn | [`xtask/src/main.rs:753`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L753) | Read the source/rustdoc for the exact contract. |
| `sync_workspace` | `private` | fn | [`xtask/src/main.rs:777`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L777) | Read the source/rustdoc for the exact contract. |
| `run_pipeline` | `private` | fn | [`xtask/src/main.rs:821`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L821) | Read the source/rustdoc for the exact contract. |
| `cleanup_workspace` | `private` | fn | [`xtask/src/main.rs:836`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L836) | Read the source/rustdoc for the exact contract. |
| `wait_child` | `private` | fn | [`xtask/src/main.rs:843`](https://github.com/ezechiel203/pcloud-rs/blob/main/xtask/src/main.rs#L843) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This package is the authoritative local build, test, coverage, packaging, qualification, and release orchestration surface; it is tooling rather than a shipped pCloud runtime library.

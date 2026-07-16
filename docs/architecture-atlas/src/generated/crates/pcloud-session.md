# `pcloud-session`

**Maturity:** Evolving product surface

**Version:** `0.1.0`

**Directory:** `crates/pcloud-session`

**Manifest:** [`crates/pcloud-session/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/Cargo.toml)

Session lifecycle, refresh-loop, and auth-vault primitives extracted from pcloud-daemon (P6.1).

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_session` | lib | [`crates/pcloud-session/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/lib.rs) |

## Direct dependencies

`pcloud-auth`, `pcloud-backends`, `pcloud-config`, `pcloud-model`, `pcloud-proto`, `pcloud-secret`, `pcloud-store`, `thiserror`

## Cargo features

No declared package features.

## File inventory (4)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-session/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-session/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/lib.rs) | library root | pcloud-session |
| [`crates/pcloud-session/src/refresh_loop.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs) | Rust module | Session refresh loop (sub-task 3). |
| [`crates/pcloud-session/src/session_lifecycle.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs) | Rust module | Daemon-side glue for session lifecycle management. |

## Rust declaration index (31 total; 17 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `refresh_loop` | `pub` | mod | [`crates/pcloud-session/src/lib.rs:44`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/lib.rs#L44) | Read the source/rustdoc for the exact contract. |
| `session_lifecycle` | `pub` | mod | [`crates/pcloud-session/src/lib.rs:45`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/lib.rs#L45) | Read the source/rustdoc for the exact contract. |
| `RefreshLoopError` | `pub` | enum | [`crates/pcloud-session/src/refresh_loop.rs:49`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L49) | Errors surfaced from one \[`tick`\] of the refresh loop. |
| `TickOutcome` | `pub` | enum | [`crates/pcloud-session/src/refresh_loop.rs:64`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L64) | The outcome of a single refresh-loop tick. Returned so the embedding runner can decide how to log / meter the… |
| `tick` | `pub` | fn | [`crates/pcloud-session/src/refresh_loop.rs:107`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L107) | Run one iteration of the session refresh loop. Expected to be invoked on a caller-owned cadence (e.g. every 6… |
| `tests` | `private` | mod | [`crates/pcloud-session/src/refresh_loop.rs:202`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L202) | Read the source/rustdoc for the exact contract. |
| `dev_runtime` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:217`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L217) | Read the source/rustdoc for the exact contract. |
| `authed_session` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:226`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L226) | Read the source/rustdoc for the exact contract. |
| `tick_noop_when_session_is_healthy` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:242`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L242) | Read the source/rustdoc for the exact contract. |
| `tick_fires_refresh_at_threshold_and_installs_fresh_token` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:257`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L257) | Read the source/rustdoc for the exact contract. |
| `tick_is_single_flight` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:287`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L287) | Read the source/rustdoc for the exact contract. |
| `tick_idle_logout_revokes_and_emits_audit_details` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:328`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L328) | Read the source/rustdoc for the exact contract. |
| `tick_hard_expiry_revokes` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:356`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L356) | Read the source/rustdoc for the exact contract. |
| `tick_returns_no_session_when_unauthenticated` | `private` | fn | [`crates/pcloud-session/src/refresh_loop.rs:376`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/refresh_loop.rs#L376) | Read the source/rustdoc for the exact contract. |
| `SessionLifecycleError` | `pub` | enum | [`crates/pcloud-session/src/session_lifecycle.rs:28`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L28) | Errors surfaced from the daemon-side lifecycle layer. |
| `SessionLifecycleConfig` | `pub` | struct | [`crates/pcloud-session/src/session_lifecycle.rs:42`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L42) | Operator-tunable lifecycle configuration. Falls back to secure defaults (1h lifetime, 80% refresh, no idle lo… |
| `SessionSupervisor` | `pub` | struct | [`crates/pcloud-session/src/session_lifecycle.rs:50`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L50) | Daemon-owned session supervisor. |
| `new` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:60`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L60) | Build a supervisor that uses the process wall clock (\[`SystemClock`\]). Prefer \[`SessionSupervisor::with_clock… |
| `with_clock` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:72`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L72) | Build a supervisor with a caller-supplied \[`Clock`\]. Used by the unit tests (via `TestClock`) to exercise thr… |
| `now_secs` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:84`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L84) | Current wall-clock seconds as observed by the supervisor's injected `Clock`. Public so \[`crate::refresh_loop:… |
| `coordinator` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:92`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L92) | Borrow the inner \[`RefreshCoordinator`\] so callers can access the single-flight guard or run raw coordinator… |
| `refresh_in_flight` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:101`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L101) | Observe whether a proactive refresh is currently holding the single-flight slot. Used by `pcloud_daemon::runt… |
| `policy` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:109`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L109) | Expose the \[`RefreshPolicy`\] so callers can attach new session lifecycles with the same timing contract the s… |
| `evaluate` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L115) | Classify the current session. Callers typically invoke this on a ticker or before each outbound API call. |
| `run_refresh` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:121`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L121) | Proactive refresh. Calls `refresh_fn` under single-flight and swaps the session token on success. |
| `handle_auth_expired` | `pub` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:137`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L137) | Handle a 401/auth-expired signal. If credentials are retained (i.e. `attach_lifecycle(..., credentials_retain… |
| `tests` | `private` | mod | [`crates/pcloud-session/src/session_lifecycle.rs:150`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L150) | Read the source/rustdoc for the exact contract. |
| `authed` | `private` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:159`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L159) | Read the source/rustdoc for the exact contract. |
| `supervisor_runs_refresh_at_threshold` | `private` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:175`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L175) | Read the source/rustdoc for the exact contract. |
| `supervisor_surfaces_auth_expired_without_retained_creds` | `private` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:203`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L203) | Read the source/rustdoc for the exact contract. |
| `supervisor_reauths_when_credentials_retained` | `private` | fn | [`crates/pcloud-session/src/session_lifecycle.rs:223`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-session/src/session_lifecycle.rs#L223) | Read the source/rustdoc for the exact contract. |

## Usage guidance

This is product code but not a frozen external library contract. Check current status and native qualification before deployment claims.

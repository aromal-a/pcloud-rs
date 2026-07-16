# `pcloud-plugin-publink-expiry`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-publink-expiry`

**Manifest:** [`crates/pcloud-plugin-publink-expiry/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/Cargo.toml)

First-party pcloud-rs plugin that emits desktop notifications when public links are about to expire.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_publink_expiry` | lib | [`crates/pcloud-plugin-publink-expiry/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs) |

## Direct dependencies

`notify-rust`, `pcloud-plugin-api`, `pcloud-secret`, `serde`, `serde_json`, `thiserror`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-publink-expiry/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-publink-expiry/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/README.md) | documentation | pcloud-plugin-publink-expiry |
| [`crates/pcloud-plugin-publink-expiry/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs) | library root | pcloud-plugin-publink-expiry |

## Rust declaration index (59 total; 27 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `CRATE_NAME` | `pub` | const | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:64`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L64) | Canonical crate identifier, used in structured logs and telemetry. |
| `DEFAULT_NOTIFY_WINDOW_HOURS` | `pub` | const | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:67`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L67) | Default notification window in hours if the operator does not override it. |
| `RATE_LIMIT_SECS` | `pub` | const | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:71`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L71) | Minimum interval between two notifications for the same link, in seconds. Fixed at 24h to avoid desktop notif… |
| `PublinkExpiryError` | `pub` | enum | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:76`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L76) | Errors surfaced by the publink expiry plugin. |
| `PublinkExpiryConfig` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:91`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L91) | Operator-supplied configuration. Mirrors the `\[plugins.publink_expiry\]` TOML table. |
| `default_enabled` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L105) | Read the source/rustdoc for the exact contract. |
| `default_window_hours` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:108`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L108) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:113`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L113) | Read the source/rustdoc for the exact contract. |
| `default_state_path` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:128`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L128) | Resolve the default state file location under `$XDG_STATE_HOME` (falling back to `$HOME/.local/state`). Retur… |
| `notify_window_secs` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:140`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L140) | Return the effective notification window in seconds. |
| `resolve_state_path` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:145`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L145) | Validate the configuration and resolve the concrete state path. |
| `NotificationState` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:163`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L163) | Persisted rate-limit state — maps `link_id` to the last unix timestamp at which a notification was emitted. |
| `state_version` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:172`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L172) | Read the source/rustdoc for the exact contract. |
| `default` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:177`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L177) | Read the source/rustdoc for the exact contract. |
| `load` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:188`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L188) | Load persisted state from `path`. Missing files become a default empty state; malformed files propagate as \[`… |
| `save` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:198`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L198) | Persist state atomically. Creates parent directories if missing. On Unix the file is created with mode `0600`. |
| `should_notify` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:231`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L231) | Should a new notification be emitted for `link_id` at `now`? This returns `true` when either the link was nev… |
| `mark_notified` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L239) | Record a notification for `link_id` at `now_unix`. |
| `Notifier` | `pub` | trait | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:250`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L250) | Platform-agnostic notifier interface. Production wiring uses \[`DesktopNotifier`\]; unit tests use \[`CapturingN… |
| `notify` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:254`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L254) | Emit a single desktop notification. Failures must NOT panic — the plugin degrades gracefully if the notificat… |
| `DesktopNotifier` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:259`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L259) | Real-desktop notifier backed by `notify-rust` on Linux/macOS/Windows. |
| `notify` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:262`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L262) | Read the source/rustdoc for the exact contract. |
| `CapturingNotifier` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:282`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L282) | In-memory notifier used by tests. Stores every `(title, body)` pair the plugin tried to emit. |
| `notify` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:288`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L288) | Read the source/rustdoc for the exact contract. |
| `Clock` | `pub` | trait | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:298`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L298) | Monotonic-ish "wall clock" trait — injected for deterministic tests. |
| `now_unix` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:300`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L300) | Return the current UNIX timestamp in seconds. |
| `SystemClock` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:305`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L305) | Production clock backed by \[`std::time::SystemTime`\]. |
| `now_unix` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:308`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L308) | Read the source/rustdoc for the exact contract. |
| `FixedClock` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:318`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L318) | Fixed clock for tests. |
| `now_unix` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:324`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L324) | Read the source/rustdoc for the exact contract. |
| `PublinkExpiryPlugin` | `pub` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:334`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L334) | The publink expiry plugin itself. |
| `fmt` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:346`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L346) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:358`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L358) | Construct a plugin using the production \[`DesktopNotifier`\] and \[`SystemClock`\]. |
| `with_parts` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:364`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L364) | Construct a plugin with an arbitrary notifier and clock — the injection point used by unit tests. |
| `notify_window_secs` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:384`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L384) | Effective notification window in seconds. |
| `state` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:390`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L390) | Read-only access to the current persisted state (primarily for tests). |
| `state_path` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:396`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L396) | Configured state file path. |
| `tick` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:403`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L403) | Enqueue the per-tick operation sequence. Called internally by \[`PublinkExpiryPlugin::tick`\] and by the host w… |
| `process_publinks` | `pub` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:415`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L415) | Core of the behaviour: given a list of link summaries and the current time, emit notifications for any link w… |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:460`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L460) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:470`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L470) | Read the source/rustdoc for the exact contract. |
| `next_operation` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:483`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L483) | Read the source/rustdoc for the exact contract. |
| `on_response` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:487`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L487) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:501`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L501) | Read the source/rustdoc for the exact contract. |
| `tmpdir` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:505`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L505) | Read the source/rustdoc for the exact contract. |
| `make_plugin` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:518`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L518) | Read the source/rustdoc for the exact contract. |
| `MtNotifier` | `private` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:536`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L536) | Thread-safe notifier that mirrors emissions into a shared buffer. |
| `notify` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:538`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L538) | Read the source/rustdoc for the exact contract. |
| `run_cycle_v2` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:548`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L548) | Helper: run one `process_publinks` cycle and return emitted notifications alongside the post-cycle persisted… |
| `expiry_within_window_emits_notification` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:570`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L570) | Read the source/rustdoc for the exact contract. |
| `expiry_outside_window_does_not_emit` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:588`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L588) | Read the source/rustdoc for the exact contract. |
| `rate_limit_suppresses_duplicate_notifications_within_24h` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:604`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L604) | Read the source/rustdoc for the exact contract. |
| `state_file_round_trip_persists_notification_state` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:637`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L637) | Read the source/rustdoc for the exact contract. |
| `disabled_config_rejects_on_load` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:657`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L657) | Read the source/rustdoc for the exact contract. |
| `next_operation_sequence_drives_timer_and_publink_list` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:680`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L680) | Read the source/rustdoc for the exact contract. |
| `on_response_publink_list_triggers_processing` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:702`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L702) | Read the source/rustdoc for the exact contract. |
| `MtN` | `private` | struct | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:712`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L712) | Read the source/rustdoc for the exact contract. |
| `notify` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:714`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L714) | Read the source/rustdoc for the exact contract. |
| `zero_window_hours_rejected_by_config` | `private` | fn | [`crates/pcloud-plugin-publink-expiry/src/lib.rs:735`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-publink-expiry/src/lib.rs#L735) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

# `pcloud-plugin-autoheal`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-autoheal`

**Manifest:** [`crates/pcloud-plugin-autoheal/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/Cargo.toml)

Auto-heal checksum scanner plugin: detects checksum mismatches, requests quarantine, and escalates repeated corruption to a full sync pause.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_autoheal` | lib | [`crates/pcloud-plugin-autoheal/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs) |
| `behaviour` | test | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs) |

## Direct dependencies

`notify-rust`, `pcloud-plugin-api`, `pcloud-secret`, `serde`, `serde_json`, `sha2`

## Cargo features

No declared package features.

## File inventory (4)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-autoheal/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-autoheal/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/README.md) | documentation | pcloud-plugin-autoheal |
| [`crates/pcloud-plugin-autoheal/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs) | library root | Auto-Heal Checksum Scanner Plugin |
| [`crates/pcloud-plugin-autoheal/tests/behaviour.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs) | test | Behavioural tests for the auto-heal plugin. All tests inject a |

## Rust declaration index (46 total; 18 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `NOTIFICATIONS_PER_PATH_PER_HOUR` | `pub` | const | [`crates/pcloud-plugin-autoheal/src/lib.rs:48`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L48) | Maximum desktop notifications per path within a rolling hour. |
| `MAX_QUARANTINES_PER_ROOT_PER_DAY` | `pub` | const | [`crates/pcloud-plugin-autoheal/src/lib.rs:52`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L52) | Maximum quarantine requests the plugin will emit per sync root in a rolling 24h window. |
| `ESCALATION_THRESHOLD` | `pub` | const | [`crates/pcloud-plugin-autoheal/src/lib.rs:56`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L56) | Number of mismatches on the same path within 24h that, once exceeded, triggers an escalation to a full sync-r… |
| `ONE_HOUR` | `private` | const | [`crates/pcloud-plugin-autoheal/src/lib.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L59) | One hour in seconds. |
| `ONE_DAY` | `private` | const | [`crates/pcloud-plugin-autoheal/src/lib.rs:61`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L61) | One day in seconds. |
| `UserResponse` | `pub` | enum | [`crates/pcloud-plugin-autoheal/src/lib.rs:67`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L67) | How the plugin reacts to a user-supplied response after a mismatch notification. Recorded for audit / observa… |
| `MismatchRecord` | `pub` | struct | [`crates/pcloud-plugin-autoheal/src/lib.rs:79`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L79) | A single historic event the plugin remembers for rate-limiting and escalation bookkeeping. |
| `Clock` | `pub` | trait | [`crates/pcloud-plugin-autoheal/src/lib.rs:92`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L92) | Abstract clock used by the plugin. Tests inject a deterministic implementation; the default uses wall-clock t… |
| `now_secs` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:94`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L94) | Current unix-seconds timestamp. |
| `SystemClock` | `pub` | struct | [`crates/pcloud-plugin-autoheal/src/lib.rs:99`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L99) | Default wall-clock implementation. |
| `now_secs` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L102) | Read the source/rustdoc for the exact contract. |
| `Notifier` | `pub` | trait | [`crates/pcloud-plugin-autoheal/src/lib.rs:112`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L112) | Abstract notifier used by the plugin. Production builds use the desktop notifier; tests inject a capturing mo… |
| `notify` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:115`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L115) | Dispatch a best-effort desktop notification. Failure is swallowed by the plugin but never escalates. |
| `DesktopNotifier` | `pub` | struct | [`crates/pcloud-plugin-autoheal/src/lib.rs:122`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L122) | Default desktop notifier backed by \[`notify_rust`\]. Construction is infallible; delivery failures are swallow… |
| `notify` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:125`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L125) | Read the source/rustdoc for the exact contract. |
| `AutoHealPlugin` | `pub` | struct | [`crates/pcloud-plugin-autoheal/src/lib.rs:141`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L141) | The auto-heal plugin. Generic over \[`Clock`\] and \[`Notifier`\] so unit tests can inject deterministic behaviou… |
| `new` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:164`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L164) | Build a production auto-heal plugin using wall-clock time and the desktop notifier. |
| `default` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:170`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L170) | Read the source/rustdoc for the exact contract. |
| `with_parts` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:178`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L178) | Build the plugin from explicit \[`Clock`\] and \[`Notifier`\] components. Primarily intended for tests. |
| `handle_event` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:194`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L194) | Handle one integrity event. Public so the host or tests can drive the plugin without routing through \[`Plugin… |
| `record_user_response` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:210`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L210) | Record the user's response to a previously-notified mismatch. Matches by (sync_root_id, path) on the most rec… |
| `recent_mismatches` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:224`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L224) | Count of mismatch events recorded for a given path in the last 24 hours. Useful for tests and host-side telem… |
| `recent_quarantines` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L239) | Count of quarantine requests the plugin has emitted for the given sync root in the last 24 hours. |
| `history` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:253`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L253) | Full audit trail of mismatch events the plugin has observed. |
| `pending_len` | `pub` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:259`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L259) | Number of operations currently queued for the host to drain. |
| `handle_mismatch` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:263`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L263) | Read the source/rustdoc for the exact contract. |
| `prune` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:347`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L347) | Read the source/rustdoc for the exact contract. |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:367`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L367) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:381`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L381) | Read the source/rustdoc for the exact contract. |
| `next_operation` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:388`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L388) | Read the source/rustdoc for the exact contract. |
| `on_response` | `private` | fn | [`crates/pcloud-plugin-autoheal/src/lib.rs:392`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/src/lib.rs#L392) | Read the source/rustdoc for the exact contract. |
| `FakeClock` | `private` | struct | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L17) | Deterministic, manually-advanced clock. `Send`-safe so the plugin continues to satisfy the plugin trait bound… |
| `new` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:22`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L22) | Read the source/rustdoc for the exact contract. |
| `advance` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:27`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L27) | Read the source/rustdoc for the exact contract. |
| `now_secs` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:33`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L33) | Read the source/rustdoc for the exact contract. |
| `CapturingNotifier` | `private` | struct | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:40`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L40) | Capturing notifier that records every (title, body) pair. |
| `count` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:45`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L45) | Read the source/rustdoc for the exact contract. |
| `notify` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:51`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L51) | Read the source/rustdoc for the exact contract. |
| `mismatch_event` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L59) | Read the source/rustdoc for the exact contract. |
| `ok_event` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:68`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L68) | Read the source/rustdoc for the exact contract. |
| `drain_ops` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:77`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L77) | Read the source/rustdoc for the exact contract. |
| `single_mismatch_emits_notification_and_quarantine` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:88`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L88) | Read the source/rustdoc for the exact contract. |
| `three_mismatches_escalate_to_full_pause` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:109`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L109) | Read the source/rustdoc for the exact contract. |
| `daily_quarantine_limit_respected` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:151`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L151) | Read the source/rustdoc for the exact contract. |
| `ok_result_does_not_escalate` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:183`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L183) | Read the source/rustdoc for the exact contract. |
| `notification_rate_limit_one_per_path_per_hour` | `private` | fn | [`crates/pcloud-plugin-autoheal/tests/behaviour.rs:201`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-autoheal/tests/behaviour.rs#L201) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

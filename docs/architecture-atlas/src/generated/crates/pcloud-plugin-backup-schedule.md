# `pcloud-plugin-backup-schedule`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-backup-schedule`

**Manifest:** [`crates/pcloud-plugin-backup-schedule/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/Cargo.toml)

User-level cron scheduler plugin that triggers backup sync cycles on a time-tick driven schedule. Supports native cron syntax and a small natural-language DSL.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_backup_schedule` | lib | [`crates/pcloud-plugin-backup-schedule/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs) |

## Direct dependencies

`chrono`, `cron`, `pcloud-plugin-api`, `serde`, `serde_json`, `thiserror`

## Cargo features

No declared package features.

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-backup-schedule/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-backup-schedule/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/README.md) | documentation | pcloud-plugin-backup-schedule |
| [`crates/pcloud-plugin-backup-schedule/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs) | library root | Backup scheduler plugin. |

## Rust declaration index (59 total; 27 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `MAX_SCHEDULES` | `pub` | const | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:51`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L51) | Maximum number of schedules a single config may contain. |
| `BackupScheduleError` | `pub` | enum | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:59`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L59) | Errors produced by the backup-schedule plugin. |
| `ScheduleEntry` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:93`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L93) | One schedule entry as materialized from `\[plugins.backup_schedule\]`. |
| `default_true` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L105) | Read the source/rustdoc for the exact contract. |
| `BackupScheduleConfig` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:111`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L111) | Full plugin configuration. |
| `validate` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:121`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L121) | Validate that the config obeys \[`MAX_SCHEDULES`\] and has unique names. Does not parse the schedule strings th… |
| `add` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:138`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L138) | Add an entry, enforcing uniqueness and the 32-schedule cap. |
| `remove` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:155`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L155) | Remove an entry by name. |
| `iter` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:165`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L165) | Iterate entries. |
| `Clock` | `pub` | trait | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:176`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L176) | A monotonic-ish wall-clock source the plugin uses to evaluate schedules. Production uses \[`SystemClock`\]; tes… |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:178`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L178) | Current wall-clock time (UTC). |
| `SystemClock` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:183`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L183) | Real system clock. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:186`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L186) | Read the source/rustdoc for the exact contract. |
| `ManualClock` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:193`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L193) | Test-controllable clock. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:199`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L199) | Start at the given UTC time. |
| `advance_secs` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:204`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L204) | Advance by `secs` seconds. |
| `set` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:209`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L209) | Jump to an absolute time. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:215`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L215) | Read the source/rustdoc for the exact contract. |
| `ParsedSchedule` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:226`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L226) | A parsed, validated schedule. |
| `as_cron` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:234`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L234) | The canonical cron expression (7-field form used by the `cron` crate internally: sec min hour dom mon dow yea… |
| `next_after` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L239) | Return the next firing time strictly after `after`, if any. |
| `parse_schedule` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:247`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L247) | Parse either a 5-field POSIX-like cron expression, a 6- or 7-field extended cron expression, or a natural-lan… |
| `looks_like_cron` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:277`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L277) | Read the source/rustdoc for the exact contract. |
| `canonicalize_cron` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:294`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L294) | Read the source/rustdoc for the exact contract. |
| `natural_to_cron` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:327`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L327) | Translate a whitelisted natural-language expression to a cron string. Grammar (case-insensitive): ```text exp… |
| `reject_trailing` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:383`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L383) | Read the source/rustdoc for the exact contract. |
| `opt_at_time` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:392`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L392) | Read the source/rustdoc for the exact contract. |
| `opt_on_dow` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:403`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L403) | Read the source/rustdoc for the exact contract. |
| `opt_on_dom` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:414`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L414) | Read the source/rustdoc for the exact contract. |
| `parse_hhmm` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:433`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L433) | Read the source/rustdoc for the exact contract. |
| `parse_dow` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:456`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L456) | Read the source/rustdoc for the exact contract. |
| `dow_name` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:471`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L471) | Read the source/rustdoc for the exact contract. |
| `RuntimeEntry` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:490`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L490) | Runtime state for a single scheduled entry. |
| `BackupSchedulePlugin` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:499`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L499) | The backup scheduler plugin. |
| `fmt` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:507`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L507) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:518`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L518) | Build a new plugin from a validated config. Uses \[`SystemClock`\]. |
| `new_with_clock` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:523`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L523) | Build with an explicit clock (used in tests). |
| `tick` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:549`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L549) | Feed a wall-clock tick to the scheduler. Any schedules whose next firing moment falls in `(last_tick, now\]` a… |
| `pending_len` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:581`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L581) | Number of operations currently queued and waiting for the host to pull them via \[`Plugin::next_operation`\]. |
| `entries` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:586`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L586) | Direct test hook: expose the current entries. |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:592`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L592) | Read the source/rustdoc for the exact contract. |
| `signature` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:601`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L601) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:605`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L605) | Read the source/rustdoc for the exact contract. |
| `next_operation` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:610`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L610) | Read the source/rustdoc for the exact contract. |
| `on_response` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:617`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L617) | Read the source/rustdoc for the exact contract. |
| `BackupScheduleCliCommand` | `pub` | enum | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:632`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L632) | Commands the CLI surface (`pcloudc backup schedule ...`) issues against the backend. Kept as a plain enum so… |
| `BackupScheduleCliReply` | `pub` | enum | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:654`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L654) | Result body the CLI returns to the user. |
| `apply_cli` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:672`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L672) | Apply a CLI command to a `BackupScheduleConfig` in memory. Persistence is the caller's responsibility — the d… |
| `_keep_timezone_imported` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:706`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L706) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:717`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L717) | Read the source/rustdoc for the exact contract. |
| `parses_cron_and_natural_expressions` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:722`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L722) | Read the source/rustdoc for the exact contract. |
| `schedule_fires_at_expected_boundaries` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:758`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L758) | Read the source/rustdoc for the exact contract. |
| `MClock` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:774`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L774) | Read the source/rustdoc for the exact contract. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:776`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L776) | Read the source/rustdoc for the exact contract. |
| `disabled_schedule_does_not_fire` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:814`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L814) | Read the source/rustdoc for the exact contract. |
| `MClock` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:816`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L816) | Read the source/rustdoc for the exact contract. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:818`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L818) | Read the source/rustdoc for the exact contract. |
| `cli_add_and_remove_persist_in_config` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:845`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L845) | Read the source/rustdoc for the exact contract. |
| `cap_of_32_enforced` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:910`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L910) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

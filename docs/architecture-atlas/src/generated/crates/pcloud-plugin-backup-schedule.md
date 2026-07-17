# `pcloud-plugin-backup-schedule`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-backup-schedule`

**Manifest:** [`crates/pcloud-plugin-backup-schedule/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/Cargo.toml)

User-level cron scheduler plugin that triggers backup sync cycles on a time-tick driven schedule. Supports native cron syntax and a small natural-language DSL.

## Feature-family profile

**Why it exists.** Add user-controlled backup timing without embedding cron parsing into the daemon core.

**What it is good for.** Cron and natural-language schedules that emit backup-cycle operations on time ticks.

**Why it is good at that job.** Pure schedule parsing plus deterministic tick handling makes time behavior independently testable.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_backup_schedule` | lib | [`crates/pcloud-plugin-backup-schedule/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs) |
| `coverage_surface` | test | [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs) |

## Direct dependencies

`chrono`, `cron`, `pcloud-plugin-api`, `serde`, `serde_json`, `thiserror`

## Cargo features

No declared package features.

## File inventory (4)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-backup-schedule/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-backup-schedule/README.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/README.md) | documentation | pcloud-plugin-backup-schedule |
| [`crates/pcloud-plugin-backup-schedule/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs) | library root | Backup scheduler plugin. |
| [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs) | test | \[derive(Clone)\] |

## Rust declaration index (64 total; 27 visible)

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
| `reject_trailing` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:388`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L388) | Read the source/rustdoc for the exact contract. |
| `opt_at_time` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:397`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L397) | Read the source/rustdoc for the exact contract. |
| `opt_on_dow` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:408`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L408) | Read the source/rustdoc for the exact contract. |
| `opt_on_dom` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:419`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L419) | Read the source/rustdoc for the exact contract. |
| `parse_hhmm` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:438`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L438) | Read the source/rustdoc for the exact contract. |
| `parse_dow` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:461`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L461) | Read the source/rustdoc for the exact contract. |
| `dow_name` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:476`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L476) | Read the source/rustdoc for the exact contract. |
| `RuntimeEntry` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:495`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L495) | Runtime state for a single scheduled entry. |
| `BackupSchedulePlugin` | `pub` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:504`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L504) | The backup scheduler plugin. |
| `fmt` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:512`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L512) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:523`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L523) | Build a new plugin from a validated config. Uses \[`SystemClock`\]. |
| `new_with_clock` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:528`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L528) | Build with an explicit clock (used in tests). |
| `tick` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:554`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L554) | Feed a wall-clock tick to the scheduler. Any schedules whose next firing moment falls in `(last_tick, now\]` a… |
| `pending_len` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:586`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L586) | Number of operations currently queued and waiting for the host to pull them via \[`Plugin::next_operation`\]. |
| `entries` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:591`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L591) | Direct test hook: expose the current entries. |
| `manifest` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:597`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L597) | Read the source/rustdoc for the exact contract. |
| `signature` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:606`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L606) | Read the source/rustdoc for the exact contract. |
| `on_load` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:610`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L610) | Read the source/rustdoc for the exact contract. |
| `next_operation` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:615`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L615) | Read the source/rustdoc for the exact contract. |
| `on_response` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:622`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L622) | Read the source/rustdoc for the exact contract. |
| `BackupScheduleCliCommand` | `pub` | enum | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:637`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L637) | Commands the CLI surface (`pcloudc backup schedule ...`) issues against the backend. Kept as a plain enum so… |
| `BackupScheduleCliReply` | `pub` | enum | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:659`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L659) | Result body the CLI returns to the user. |
| `apply_cli` | `pub` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:677`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L677) | Apply a CLI command to a `BackupScheduleConfig` in memory. Persistence is the caller's responsibility — the d… |
| `_keep_timezone_imported` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:711`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L711) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:722`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L722) | Read the source/rustdoc for the exact contract. |
| `parses_cron_and_natural_expressions` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:727`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L727) | Read the source/rustdoc for the exact contract. |
| `schedule_fires_at_expected_boundaries` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:763`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L763) | Read the source/rustdoc for the exact contract. |
| `MClock` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:779`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L779) | Read the source/rustdoc for the exact contract. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:781`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L781) | Read the source/rustdoc for the exact contract. |
| `disabled_schedule_does_not_fire` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:819`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L819) | Read the source/rustdoc for the exact contract. |
| `MClock` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:821`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L821) | Read the source/rustdoc for the exact contract. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:823`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L823) | Read the source/rustdoc for the exact contract. |
| `cli_add_and_remove_persist_in_config` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:850`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L850) | Read the source/rustdoc for the exact contract. |
| `cap_of_32_enforced` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/src/lib.rs:915`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/src/lib.rs#L915) | Read the source/rustdoc for the exact contract. |
| `SharedClock` | `private` | struct | [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs:15`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs#L15) | Read the source/rustdoc for the exact contract. |
| `now` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs:18`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs#L18) | Read the source/rustdoc for the exact contract. |
| `entry` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs:23`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs#L23) | Read the source/rustdoc for the exact contract. |
| `public_schedule_parser_covers_canonical_and_rejection_surface` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs:33`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs#L33) | Read the source/rustdoc for the exact contract. |
| `config_clock_cli_and_plugin_contract_cover_success_and_failures` | `private` | fn | [`crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs:100`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-backup-schedule/tests/coverage_surface.rs#L100) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

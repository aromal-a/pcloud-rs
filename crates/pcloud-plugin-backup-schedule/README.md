# pcloud-plugin-backup-schedule

Wave H9. First-party, single-user pcloud-rs plugin that fires a
timer-tick event for configured sync roots on a user-defined
schedule.

Authoritative user docs:
[`docs/plugins/backup-schedule.md`](../../docs/plugins/backup-schedule.md).

## Purpose

A small, in-process cron living inside the daemon. It exists so
users can get scheduled sync-resumes on hosts where they cannot (or
do not want to) drop a systemd timer, launchd plist, Task Scheduler
entry, or crontab — laptops, minimal-systemd BSD hosts, locked-down
corp machines.

The plugin **itself does not create snapshots**. It only emits a
per-tick event and asks the host to resume the configured sync root.
Actual backup snapshot creation is performed by the backup CLI
subcommands (`pcloudc backup snapshot-create`, etc.). Users who want
scheduled snapshots should wire their cron entry to call that CLI,
or use an external scheduler if the daemon cron is not enough.

## Plugin-API ops used

The plugin uses existing ops only — it does not introduce new ones:

- `PluginOperation::RequestSyncResume { sync_root_id }`
- `PluginOperation::TimerTick { period_secs }`

Both are in `pcloud-plugin-api`.

## Capabilities

| Capability        | Required |
|-------------------|:--------:|
| `ObserveStatus`   | no       |
| `SyncControl`     | yes      |
| `CryptoControl`   | no       |
| `NetworkEgress`   | no       |

`SyncControl` requires `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1` plus the
master `PCLOUD_PLUGINS_ENABLED=1`.

## Configuration knobs

`[plugins.backup_schedule]`:

| Key        | Type                 | Default | Purpose                             |
|------------|----------------------|---------|-------------------------------------|
| `enabled`  | bool                 | `true`  | Master switch.                      |
| `entries`  | array of `[[...]]`   | `[]`    | Schedule entries (cap 32).          |

Entry fields:

| Key            | Type    | Required | Notes                                 |
|----------------|---------|:--------:|---------------------------------------|
| `name`         | string  | yes      | Unique, stable, user-visible.         |
| `schedule`     | string  | yes      | Cron expression or natural DSL.       |
| `sync_root_id` | u64     | yes      | From `pcloudc sync list`.             |
| `enabled`      | bool    | no       | Per-entry toggle; default `true`.     |

## Schedule DSL

Accepted:

- **Cron**: 5/6/7-field POSIX cron (e.g. `0 18 * * 5`).
- **Natural language**, whitelisted grammar only:
  `hourly`, `daily [at HH:MM]`, `weekly [on <day>] [at HH:MM]`,
  `monthly [on 1..=31] [at HH:MM]`, `every <day> [at] HH:MM`.

Anything outside that grammar is rejected. The DSL is deliberately
tiny so it can never accidentally become a shell-like expression
language.

## Internal traits

- `Clock` — abstracts wall time; `ManualClock` drives deterministic
  boundary-crossing tests.

## Tick semantics

On each daemon time-tick the plugin evaluates every enabled entry.
If the next firing moment falls in `(last_tick, now]`, a
`RequestSyncResume` is enqueued. Per-entry per-tick firings are
capped at 1024 as a catch-up guard (laptops suspended for days).

**Sleep replay policy**: on wake, the plugin fires **once**, not
once per missed slot, to avoid stampedes.

## Security posture

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`.
- No secrets of any kind.
- Only `SyncControl` requested; cannot pause sync, inspect crypto,
  or make network calls.
- 32-entry cap + 1024-per-tick cap prevent pathological configs
  from degrading daemon responsiveness.

## Single-user scope

Schedules are per local install, scoped to the currently logged-in
account's sync roots. No fleet-wide rollout, no central scheduling
service.

## Honest limitations

- **Fires a tick, does not snapshot.** The plugin only asks the
  host to resume a sync root. Actual backup snapshot creation is
  done by the `pcloudc backup snapshot-*` CLI; wire those in
  externally if you need scheduled encrypted snapshots.
- **Resume, not pause.** Cannot schedule pauses. Scope the plugin
  minimally; a future, separate plugin may add pause scheduling.
- **No run-once.** Every entry is recurring. Use `at(1)` or a
  one-shot timer for one-off runs.
- **Clock jumps.** Manual clock jumps backwards can retrigger an
  entry; the 1024-per-tick cap prevents pathological loops. Use NTP.
- **Entry cap.** 32 entries is a guard-rail; open a tracker issue
  if you genuinely need more.

## BNF-ish grammar for the natural DSL

```
expr   := "hourly"
        | "daily"   [ "at" HH:MM ]
        | "weekly"  [ "on"  DAY ] [ "at" HH:MM ]
        | "monthly" [ "on"  DOM ] [ "at" HH:MM ]
        | "every"   DAY [ "at" ] HH:MM

DAY    := mon|tue|wed|thu|fri|sat|sun (full names also accepted)
DOM    := 1..=31
HH     := 0..=23
MM     := 0..=59
```

Verbs outside this whitelist are rejected at parse time. See
[`docs/plugins/backup-schedule.md`](../../docs/plugins/backup-schedule.md)
for ten worked examples.

## Lifecycle (dev summary)

- On each daemon tick `BackupSchedulePlugin::tick()` walks from
  `last_tick` through `now`, enqueuing one `RequestSyncResume` per
  boundary crossed.
- Per-entry per-tick firings are capped at 1024 to bound catch-up
  after long suspensions.
- Wake-from-sleep fires **once**, not once per missed slot.
- `Clock` trait is the sole abstraction around wall time; tests
  inject `ManualClock`.

## Tests

```bash
cargo test -p pcloud-plugin-backup-schedule
```

5 tests: DSL+cron parsing, boundary-crossing firing, disabled-entry
no-op, CLI persist, and the 32-entry cap.

### Manual smoke test

```bash
export PCLOUD_PLUGINS_ENABLED=1
export PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1

NEXT=$(( ( $(date +%M) + 2 ) % 60 ))
pcloudc backup schedule add "smoke" "${NEXT} * * * *" <sync_root_id>
pcloudc --field next_fire --json backup schedule list
```

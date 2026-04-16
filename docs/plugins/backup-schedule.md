# Backup Schedule Plugin

Crate: `pcloud-plugin-backup-schedule` (wave H9)

## 1. Purpose

The backup-schedule plugin runs inside the pcloud-rs daemon and triggers
`PluginOperation::RequestSyncResume` on configured sync roots according
to a user-defined schedule. It is essentially a small, user-level cron
that lives inside the daemon process — no external cron daemon, systemd
timer, launchd plist, or Task Scheduler entry is required, which matters
on laptops, BSD/minimal-systemd hosts, and corp machines where users
cannot drop arbitrary unit files.

It is loaded statically into the daemon binary like every other
plugin in this workspace; enabling it is a config toggle, not a load
step.

## 2. Why this plugin exists — the incident class it prevents

- Users set a nightly backup cron entry, the cron daemon is replaced
  by a systemd timer they do not have permission to touch, and the
  backup silently stops running.
- Laptop users who rely on `/etc/cron.d/*` miss every firing whose
  moment fell during sleep; the `anacron`-style compensation requires
  root.
- Users on immutable or appliance-style distributions (Silverblue,
  NixOS, ChromeOS Crostini, locked corporate images) have no user-level
  scheduler at all.

Running the schedule inside the daemon eliminates the external moving
part. A single `pcloudc backup schedule add` command is enough.

**The plugin itself does not create snapshots.** It only emits a
per-tick event and asks the host to resume the configured sync root.
Actual backup snapshot creation is performed by the backup CLI
subcommands (`pcloudc backup snapshot-create`, etc.). Users who want
scheduled snapshots should wire their cron entry to call that CLI, or
chain the CLI invocation after a scheduled resume.

## 3. Capabilities

| Capability        | Required | Purpose                                       |
|-------------------|:--------:|-----------------------------------------------|
| `ObserveStatus`   | no       | Does not read status.                         |
| `SyncControl`     | yes      | Issues `RequestSyncResume` on its schedule.   |
| `CryptoControl`   | no       | Never touches key material.                   |
| `NetworkEgress`   | no       | Never opens sockets.                          |

Because `SyncControl` is required, operators must also set
`PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1` in addition to the master
`PCLOUD_PLUGINS_ENABLED=1`. Capability grants are still enforced by
`pcloud-plugin-api`'s registry at every dispatch, so a compromised or
buggy config cannot cause the plugin to do more than resume sync.

**Runtime-gated enforcement.** Every `RequestSyncResume` call goes
through `PluginRegistry::dispatch`, which re-checks `SyncControl` on
the granted set before the host handler executes. A revocation
emits `plugin.capability.denied{plugin=backup_schedule, op=request_sync_resume,
missing=SyncControl}`. A panicking schedule tick is caught by the
registry's `catch_unwind` guard; the plugin is de-registered and the
daemon continues to run.

## 4. Configuration reference

`[plugins.backup_schedule]` in `pcloud.conf`:

| Key        | Type                 | Default | Validation                          | Purpose                             |
|------------|----------------------|---------|-------------------------------------|-------------------------------------|
| `enabled`  | bool                 | `true`  | —                                   | Master switch.                      |
| `entries`  | array of `[[…]]`     | `[]`    | len ≤ 32; names unique; schedules parse | Schedule entries.               |

Per-entry fields (`[[plugins.backup_schedule.entries]]`):

| Key            | Type    | Required | Validation                                | Notes                                 |
|----------------|---------|:--------:|-------------------------------------------|---------------------------------------|
| `name`         | string  | yes      | non-empty; unique within `entries`        | Stable, user-visible identifier.      |
| `schedule`     | string  | yes      | parses as cron or natural DSL             | See grammar below.                    |
| `sync_root_id` | u64     | yes      | must exist in `pcloudc sync list`         | Numeric id of the sync root.          |
| `enabled`      | bool    | no       | —                                         | Per-entry toggle; default `true`.     |

Full example:

```toml
[plugins.backup_schedule]
enabled = true

[[plugins.backup_schedule.entries]]
name         = "friday-backup"
schedule     = "every friday 18:00"
sync_root_id = 42
enabled      = true

[[plugins.backup_schedule.entries]]
name         = "nightly-docs"
schedule     = "0 2 * * *"
sync_root_id = 17
```

## 5. Schedule DSL

Two forms are accepted. Both are parsed into a canonical 7-field cron
string and executed by the `cron` crate internally.

### 5.1 Cron

POSIX-style 5-field cron, plus the 6- and 7-field extensions supported
by the `cron` crate.

```
minute hour day-of-month month day-of-week          # 5-field
sec    min  hour day-of-month month day-of-week     # 6-field
sec    min  hour day-of-month month day-of-week yr  # 7-field
```

Example: `0 18 * * 5` — every Friday at 18:00.

### 5.2 Natural DSL — BNF-ish grammar

```
expr        := "hourly"
             | "daily"    [ "at" HH:MM ]
             | "weekly"   [ "on"  DAY ] [ "at" HH:MM ]
             | "monthly"  [ "on"  DOM ] [ "at" HH:MM ]
             | "every"    DAY [ "at" ] HH:MM

DAY         := "monday"    | "mon"
             | "tuesday"   | "tue"
             | "wednesday" | "wed"
             | "thursday"  | "thu"
             | "friday"    | "fri"
             | "saturday"  | "sat"
             | "sunday"    | "sun"

DOM         := 1..=31                   ; integer, clamped by cron

HH          := 0..=23
MM          := 0..=59
HH:MM       := HH ":" MM                ; two-digit fields, zero-padded

WS          := " " | "\t"               ; any run of whitespace separates tokens
```

The verb set — `hourly`, `daily`, `weekly`, `monthly`, `every`, plus the
connectives `at`, `on` — is a **hard whitelist**. Anything outside this
set is a parse error. The DSL is deliberately tiny so it cannot
accidentally become a shell-like expression language.

### 5.3 Ten worked examples

| # | Schedule expression                  | Canonical cron        | Meaning                                          |
|---|--------------------------------------|-----------------------|--------------------------------------------------|
| 1 | `hourly`                             | `0 0 * * * * *`       | Top of every hour.                               |
| 2 | `daily at 03:00`                     | `0 0 3 * * * *`       | 03:00 every day.                                 |
| 3 | `daily at 23:30`                     | `0 30 23 * * * *`     | 23:30 every day (before bedtime).                |
| 4 | `weekly on monday at 09:15`          | `0 15 9 * * MON *`    | 09:15 every Monday.                              |
| 5 | `weekly on sun at 06:00`             | `0 0 6 * * SUN *`     | 06:00 every Sunday.                              |
| 6 | `monthly on 1 at 00:00`              | `0 0 0 1 * * *`       | Midnight on the 1st of each month.               |
| 7 | `every friday 18:00`                 | `0 0 18 * * FRI *`    | 18:00 every Friday.                              |
| 8 | `every wed at 12:00`                 | `0 0 12 * * WED *`    | 12:00 every Wednesday.                           |
| 9 | `0 2 * * *`                          | `0 0 2 * * * *`       | 02:00 every day (5-field cron).                  |
| 10 | `*/15 * * * *`                      | `0 */15 * * * * *`    | Every 15 minutes (cron expression).              |

Values such as `run every minute` or `every 5 seconds` are rejected —
they fall outside the whitelisted grammar. If you need sub-hour
frequency, use a cron expression.

## 6. Tick semantics — boundary-crossing

On each daemon time-tick the plugin evaluates every enabled entry.
Let `last_tick` be the plugin's recorded tick timestamp and `now` the
current instant. For each entry:

1. Compute `next = entry.schedule.next_after(last_tick)`.
2. Walk forward from `last_tick`, enqueuing one
   `RequestSyncResume { sync_root_id }` per boundary moment that falls
   in `(last_tick, now]`.
3. Stop when `next > now`, or when the per-entry catch-up cap of
   **1024 boundaries in a single tick** is reached.
4. Set `last_tick = now`.

Host behaviour when a `RequestSyncResume` arrives:

- Sync root is paused → a single fresh cycle runs.
- Sync root is already running → the request is a no-op from the
  host's point of view.

### Sleep replay policy

If the host is suspended across several scheduled firings (laptop lid
closed for a weekend), the plugin fires **once** on wake — not once
per missed slot. The catch-up cap exists to bound this; the design
goal is "no stampede on wake".

### Clock trait — for testability

The plugin depends on a `Clock` trait (see
`crates/pcloud-plugin-backup-schedule/src/lib.rs`). Production uses
`SystemClock`; tests inject `ManualClock` and call
`clock.advance_secs(n)` to prove boundary crossings fire. Do not
call `plugin.tick()` by hand from real code — the host does it from
its poll loop.

## 7. Outputs

- **IPC operation** per fired boundary:
  `RequestSyncResume { sync_root_id }`.
- **Audit log**: one entry per fired boundary with source
  `pcloud-rs.backup-schedule` (via the host's audit sink).
- **No desktop notification**. Backups should not pop a toast.
- **CLI `list` response**: `BackupScheduleCliReply::List { entries }`
  for the current configured set.

## 8. CLI interactions

Managed via `pcloudc backup schedule`:

```
pcloudc backup schedule list
pcloudc backup schedule add "<name>" "<when>" <sync_root_id>
pcloudc backup schedule remove <name>
```

Example session:

```bash
pcloudc sync list
pcloudc backup schedule add "nightly-docs"   "0 2 * * *"              17
pcloudc backup schedule add "friday-backup"  "every friday 18:00"     42
pcloudc backup schedule add "weekend-photos" "weekly on sat at 06:00" 77
pcloudc backup schedule list
pcloudc --json backup schedule list     # raw JSON envelope for scripts
```

The CLI serialises a `BackupScheduleCliCommand` over the existing
daemon IPC and receives a `BackupScheduleCliReply`. Mutations are
persisted to the daemon's config store, so entries survive restarts
without the user editing TOML by hand.

## 9. Test recipes

### Unit tests (built-in)

```bash
cargo test -p pcloud-plugin-backup-schedule
```

Covered: DSL + cron parsing across all grammar branches, boundary
firing under `ManualClock`, disabled-entry no-op, CLI add/remove
persistence, 32-entry cap.

### Manual verification

```bash
export PCLOUD_PLUGINS_ENABLED=1
export PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1

# 1. add a near-future schedule
pcloudc sync list                       # note the sync_root_id
NOW=$(date +%M); NEXT=$(( (NOW + 2) % 60 ))
pcloudc backup schedule add "smoke" "${NEXT} * * * *" <sync_root_id>

# 2. observe
pcloudc backup schedule list
watch -n5 'pcloudc status'              # wait for the boundary

# 3. confirm a RequestSyncResume fired
pcloudc --json audit verify | jq '.[] | select(.source == "pcloud-rs.backup-schedule")'
```

### Field-selector probe

```bash
# Just the next-firing time for each entry
pcloudc --field next_fire --json backup schedule list
```

## 10. Failure modes

| Symptom                               | Cause                                       | Remedy                                              |
|---------------------------------------|---------------------------------------------|-----------------------------------------------------|
| Schedule never fires                  | Entry `enabled = false`                      | Flip to `true` or use CLI to re-add.                |
| Schedule never fires, entry enabled   | Host suspended past the boundary; wake too early for `tick()` to observe | Wait for the next boundary; sleep replay fires once on wake. |
| `InvalidSchedule("...")` at startup   | Config string outside grammar                | Fix the string or switch to a cron expression.     |
| `DuplicateName("...")`                | Two entries share a `name`                   | Rename one entry.                                   |
| `TooMany { .. }`                      | More than 32 entries                         | Consolidate or file an issue.                       |
| Fires far more than expected           | Clock was manually stepped backwards         | Stop messing with the clock; use NTP.               |

## 11. Limitations (honest)

- **Resume, not pause.** The plugin can only ask the host to resume a
  sync root. It does not pause syncs on its own — if you want
  scheduled *pauses* too, that belongs in a separate plugin or in the
  host's scheduler. This is a capability-minimisation choice, not an
  oversight.
- **No run-once.** Every entry is recurring. To run a sync exactly
  once at a target time, use `at(1)` or a one-shot systemd timer and
  call `pcloudc resume` directly.
- **Laptop sleep catch-up.** If the host is suspended across several
  scheduled firings, the plugin fires **once** on wake rather than
  replaying every missed slot. This is intentional — replays would
  trigger a stampede on resume.
- **Clock changes.** Manual clock jumps backwards can cause an entry
  to fire again. The 1024-per-tick cap prevents pathological loops.
  Use NTP.
- **Entry cap.** The 32-entry cap is a guard-rail, not a user limit
  in disguise; if you genuinely need more than 32 schedules, open a
  tracker issue rather than patching the constant.
- **No snapshot orchestration.** The plugin does not create backup
  snapshots; it only emits `RequestSyncResume`. Wire
  `pcloudc backup snapshot-create` separately if you need snapshots.
- **Single-user scope.** No fleet-wide rollout, no central scheduling
  service.

## 12. Tuning: home deploy vs. FAANG enterprise

| Concern / knob                   | Home / single-user                   | Enterprise / fleet                                 |
|----------------------------------|--------------------------------------|----------------------------------------------------|
| Number of entries                 | Typically 1–3                        | Use enterprise fleet tooling to manage per-host schedules; keep the local plugin simple |
| Schedule format                  | Natural DSL (`daily at 02:00`)        | Cron expressions, committed to config repo        |
| Catch-up on wake                 | One fire per wake (default)          | Same (no knob today); replay logic is intentional  |
| Audit forwarding                 | Local log                            | Forward `pcloud-rs.backup-schedule` events to SIEM |
| Paired with snapshots?           | Usually not                          | Yes — wire `pcloudc backup snapshot-create` behind the firing |
| Clock discipline                 | NTP recommended                       | NTP or a managed time source is mandatory         |

## 13. Security posture

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]` crate-wide.
- The plugin carries no secrets; config is plain cron/DSL text.
- Only `SyncControl` is requested. It cannot pause syncs, query
  crypto state, or make network calls.
- The 32-entry cap and 1024-per-tick cap prevent pathological configs
  from degrading daemon responsiveness.

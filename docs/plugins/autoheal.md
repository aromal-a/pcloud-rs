# Auto-Heal Checksum Scanner Plugin

Crate: `pcloud-plugin-autoheal` (wave H8)

## 1. Purpose

`pcloud-plugin-autoheal` is a first-party pcloud-rs plugin that turns
the daemon's file-integrity scanner into an **actionable** corruption
response loop. On its own the integrity scanner is a passive observer:
it hashes files, compares the result against the recorded metadata,
and writes the outcome into the audit stream. Useful, but not load-bearing
— a user who never reads the audit log will never learn that a file has
silently rotted. The auto-heal plugin closes that loop.

## 2. Why this plugin exists — the incident class it prevents

Silent data corruption is the class of bug users are worst at
noticing:

- A flaky SATA cable flips bits on one copy of a 2 GiB photo tarball
  every few reads; the sync daemon faithfully pushes the corrupted
  copy upstream, and every host that re-syncs pulls garbage.
- A filesystem bug (e.g. a kernel regression on an experimental ZFS
  pool) causes random small-file corruption in `~/Documents`.
- A third-party "optimiser" utility rewrites JPEG EXIF data in place,
  invalidating the recorded content hash.
- bit-rot on aging SSDs manifests as sporadic single-bit errors.

In all of these cases, the daemon's integrity scanner can *detect* the
mismatch immediately, but nobody is looking at the audit log. Without
an actionable plugin in the loop, a corrupt file keeps syncing until a
human notices, often weeks later, via a broken download.

This plugin closes the gap:

1. Shows a rate-limited desktop notification for each `Mismatch` so
   the user sees the problem immediately.
2. Asks the host to **quarantine** the affected sync root so the bad
   copy is not propagated further.
3. If the same path mismatches more than 3 times in 24h, escalates
   to a full `RequestSyncPause` for that root, on the assumption
   something structurally wrong is underway (failing disk, a background
   rewriter, a partially-mounted ZFS pool, etc.).

The plugin is **not a repair tool.** It never fetches a pristine copy
from the server and overwrites the bad local one. Automated rewrite on
top of a filesystem you cannot yet trust is the wrong default.

## 3. Capabilities

| Capability        | Required | Purpose                                                |
|-------------------|:--------:|--------------------------------------------------------|
| `ObserveStatus`   | yes      | Subscribe to `ObserveIntegrityEvents`.                 |
| `SyncControl`     | yes      | Issue `RequestQuarantine` and `RequestSyncPause`.      |
| `CryptoControl`   | no       | Never touches key material.                            |
| `NetworkEgress`   | no       | No network I/O.                                        |

Because `SyncControl` is required, operators must also set
`PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1` in addition to
`PCLOUD_PLUGINS_ENABLED=1`. Both are off by default.

**Runtime-gated enforcement.** Every `RequestQuarantine`,
`RequestSyncPause`, and `RequestSyncResume` from autoheal passes
through `PluginRegistry::dispatch` and is re-checked against the
granted capability set on every call. If `SyncControl` is revoked at
runtime, quarantine/pause requests are dropped before the host
handler executes; the registry emits
`plugin.capability.denied{plugin=autoheal, op=request_quarantine,
missing=SyncControl}` and returns `PluginError::CapabilityNotGranted`
to the caller. A panicking handler is isolated by the registry
(`catch_unwind`) and autoheal is de-registered until the next daemon
restart.

## 4. Configuration reference

`[plugins.autoheal]` in `pcloud.conf`:

| Key                               | Type | Default | Validation | Purpose                                   |
|-----------------------------------|------|--------:|------------|-------------------------------------------|
| `enabled`                         | bool | `true`  | —          | Master switch.                            |
| `escalation_threshold_per_path`   | u32  | `3`     | `≥ 1`      | Mismatches/24h before escalation to pause. |
| `max_quarantines_per_sync_root`   | u32  | `10`    | `≥ 0`      | Quarantine requests/24h per sync root.    |
| `notification_cooldown_seconds`   | u32  | `3600`  | `≥ 0`      | Per-path desktop-notify cooldown.         |

All fields have safe defaults and may be omitted. `enabled = false`
de-registers the plugin entirely at daemon startup.

Escalations are additionally hard-capped at **1 per sync root per 24h**
(not a config knob — this prevents runaway loops).

## 5. Lifecycle + event flow

```
 on_load ──────▶ ObserveIntegrityEvents (emitted once)
                           │
                           ▼
                  ┌────────────────────┐
                  │  host streams      │
                  │ FileIntegrityResult│
                  └──────────┬─────────┘
                             │
                  outcome == Mismatch?
                             │
                 no ◀────────┼────────▶ yes
                             │            │
                     (ignore)│            ├─▶ notify (≤ 1/path/hour)
                             │            │
                             │            ├─▶ quarantine (≤ 10/root/day)
                             │            │
                             │            │   history[path]++ within 24h
                             │            │
                             │            ▼
                             │    history[path] > 3?
                             │            │
                             │            │─── yes ──▶ RequestSyncPause
                             │            │             (≤ 1/root/day)
                             │            │
                             │            └── no ───▶ (wait for next event)
                             ▼
                    ┌────────────────────┐
                    │ sliding 24h window │
                    │    prune on each   │
                    │       event        │
                    └────────────────────┘
```

## 6. Rule taxonomy (event classification)

The plugin observes every `FileIntegrityResult` and classifies by
`outcome`:

| `FileIntegrityOutcome` | Plugin action                                     |
|------------------------|---------------------------------------------------|
| `Ok`                   | Ignored.                                          |
| `Unreadable`           | Ignored (usually transient, e.g. a file being written).  |
| `Mismatch`             | Notify + quarantine + (maybe) escalate.           |

The plugin deliberately does not react to `Unreadable`. Users who open
a file for writing while the scanner runs will otherwise generate a
flood of notifications.

## 7. Outputs

- **Desktop notification** (best-effort, rate-limited; uses
  `notify-rust` on Linux / macOS / Windows). Headless hosts skip this
  step silently.
- **IPC operations** emitted to the host:
  - `PluginOperation::RequestQuarantine { sync_root_id, path }`
  - `PluginOperation::RequestSyncPause { sync_root_id }` (escalation)
- **Audit log**: every handled mismatch is recorded by the host's
  audit engine as a standard integrity event plus the plugin's
  reaction.
- **In-memory history**: `history()` returns the plugin's recent
  decisions; exposed to tests and to the `pcloudc doctor` bundle.

## 8. Test recipes

### Unit tests (built-in)

```bash
cargo test -p pcloud-plugin-autoheal
```

Covered: single-mismatch notify+quarantine, escalation to pause at
threshold, daily quarantine cap, `Ok` outcome does not escalate,
notification rate-limit (1/path/hour).

### Manual verification

```bash
export PCLOUD_PLUGINS_ENABLED=1
export PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1

# 1. ensure autoheal is enabled
pcloudc config set plugins.autoheal.enabled true

# 2. corrupt a known file *after* it has been hashed by the scanner
printf 'bitrot\n' >> ~/pcloud/docs/report.pdf

# 3. trigger a fresh integrity sweep (depends on the scanner cadence)
pcloudc sync localscan

# 4. observe
pcloudc status                     # sync root should show quarantine
pcloudc --field kind --json audit  # integrity + autoheal events
```

Repeat step 2 three times within 24h to see the escalation path fire.

### Field-selector probe

```bash
# Only surface autoheal-driven events
pcloudc --json audit verify | jq '.[] | select(.source == "pcloud-rs.autoheal")'
```

## 9. Failure modes

| Symptom                                | Cause                                      | Remedy                                            |
|----------------------------------------|--------------------------------------------|---------------------------------------------------|
| No desktop popup on a known mismatch   | Headless host / no D-Bus                    | Expected. Audit log still records the event.      |
| Sync root stuck in "paused"             | Plugin escalated to pause                   | `pcloudc resume <root>` once the underlying issue is fixed. |
| Notification flood                     | Cooldown configured to 0                    | Raise `notification_cooldown_seconds`.             |
| Never escalates even after many mismatches | `escalation_threshold_per_path` set high | Lower it; the default of 3 is usually correct.    |
| Quarantine cap seems to silence alarms | `max_quarantines_per_sync_root` hit         | This is by design; the sync root is already being hammered. |

## 10. Limitations (honest)

- **Not a repair tool.** The plugin pauses and quarantines; it does
  not fetch a pristine copy from the server and replace the bad one
  on disk. Repair is deliberately left to explicit user action.
- **Pauses are sticky.** Once the plugin escalates a sync root to a
  pause, the user must explicitly resume it. Silent auto-resume would
  hide an ongoing integrity problem.
- **Notifications are best-effort.** On headless hosts (CI, servers,
  remote SSH) there is no notifier, and the plugin silently skips
  that step. Quarantine/pause actions still happen; they are visible
  via `pcloudc status` and the audit stream.
- **Per-account scope.** The plugin operates on the currently
  logged-in account; it does not reason across accounts.
- **Advisory only.** No central control plane; local host only.
- **No cross-host coordination.** A second host syncing the same
  corrupt file will make its own independent decisions.

## 11. Tuning: home deploy vs. FAANG enterprise

| Knob                            | Home / single-user     | Enterprise / fleet (single host still) |
|---------------------------------|------------------------|----------------------------------------|
| `escalation_threshold_per_path` | 3 (default)             | 2 — escalate sooner, integrate with on-call |
| `max_quarantines_per_sync_root` | 10 (default)            | Tune lower if you have tight alert budgets |
| `notification_cooldown_seconds` | 3600 (default)          | 300 (or lower) if operators want fine-grained visibility |
| Audit forwarding                | Local log               | Ship audit log to SIEM; `autoheal` events include `source = "pcloud-rs.autoheal"` |
| Escalation response             | Manual `pcloudc resume` | Automated runbook in your incident system |

For fleet-wide integrity auditing, pair with the enterprise data-residency
and fleet modules — the plugin itself is single-host by design.

## 12. Security posture

- `#![forbid(unsafe_code)]` and `#![deny(missing_docs)]`.
- Only non-secret data (`path`, `sync_root_id`, outcome enum) ever
  crosses the plugin boundary — `FileIntegrityResult` intentionally
  has no secret fields.
- Desktop notification failures (no D-Bus, headless CI, etc.) are
  swallowed and never escalate to a plugin error — the integrity
  pipeline must not stall because the host happens to be headless.
- The plugin never reads from disk, never touches the network, and
  never persists anything outside its in-memory state. Audit history
  is available to the host through `history()` for durable logging.

## 13. CLI interactions

The plugin does not introduce new CLI subcommands. The user-facing
remedies come from the existing CLI surface:

- `pcloudc status` — see which sync roots are currently quarantined
  or paused.
- `pcloudc resume` / `pcloudc sync resume` — lift a pause the plugin
  put in place once you have investigated.
- `pcloudc sync localscan` — force a rescan once the underlying
  issue has been fixed.
- `pcloudc doctor` — surfaces the most recent integrity events and
  plugin history.

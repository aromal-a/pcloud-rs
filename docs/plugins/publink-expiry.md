# Publink Expiry Notifier Plugin

Crate: `pcloud-plugin-publink-expiry` (wave H7)

## 1. Purpose

`pcloud-plugin-publink-expiry` is a first-party, single-user pcloud-rs
plugin that raises a desktop notification when one of your pCloud
public links is about to expire. It exists to keep operators from
being surprised when a shared link silently dies.

The plugin is pull-driven by the pcloud-rs daemon: it does **not** make
any network calls of its own, it does **not** receive any secrets, and
it requests exactly one capability (`ObserveStatus`). All link data
comes from the daemon through the typed
`PluginOperation::ObservePublinkList` boundary, which returns a
redacted `PublinkSummary` — no passwords, no owner ids, no short-link
URLs. The plugin only ever sees `link_id`, kind, and expiry timestamp.

It is a statically-linked in-process plugin like every other plugin in
this tree. There is no dynamic loader; enabling it is a flag in the
daemon config.

## 2. Why this plugin exists — the incident class it prevents

"Did the link expire?" is effectively the first troubleshooting
question anyone asks when a recipient reports a broken pCloud share:

- A user sets an expiry one year out, forgets the exact date, and
  the link silently dies a year later when a customer tries to
  download.
- A contractor rotates their shares quarterly via an ad-hoc reminder
  in a personal calendar; the reminder gets lost in a calendar import.
- A shared folder's auto-expire policy is set correctly, but the
  downstream recipient has no way to know it until the download 404s.

All the user needs is a local popup a day before the link dies. That
is exactly what this plugin is — nothing more.

It is **advisory only**. It does not renew, rotate, or revoke links.

## 3. Capabilities

| Capability        | Required | Purpose                                             |
|-------------------|:--------:|-----------------------------------------------------|
| `ObserveStatus`   | yes      | Subscribe to `ObservePublinkList`.                  |
| `SyncControl`     | no       | Never pauses or resumes sync.                       |
| `CryptoControl`   | no       | Never touches key material.                         |
| `NetworkEgress`   | no       | All I/O goes through the host; no sockets opened.   |

The default `ExtensionPolicy` always permits `ObserveStatus`, so this
plugin runs with no extra `PCLOUD_PLUGIN_ALLOW_*` environment flags —
only the master `PCLOUD_PLUGINS_ENABLED=1` is needed.

**Runtime-gated enforcement.** `ObservePublinkList` dispatches through
`PluginRegistry::dispatch`, which re-verifies `ObserveStatus` on every
call. A revocation produces `plugin.capability.denied{plugin=publink_expiry,
op=observe_publink_list, missing=ObserveStatus}` and the notifier tick
is dropped. A panicking tick is isolated by the registry (catch_unwind)
and the plugin is de-registered for the rest of the daemon's lifetime.

## 4. Configuration reference

`[plugins.publink_expiry]` in `pcloud.conf`:

| Key                    | Type   | Default            | Validation                 | Purpose                               |
|------------------------|--------|--------------------|----------------------------|---------------------------------------|
| `enabled`              | bool   | `true`             | —                          | Master switch.                        |
| `notify_window_hours`  | u32    | `24`               | `> 0`                      | How far before expiry to warn.        |
| `state_file`           | path   | XDG-derived        | Parent writable by daemon  | Override rate-limit JSON location.    |

State file defaults:

- Linux: `$XDG_STATE_HOME/pcloud-rs/publink-expiry.json` (falls back to
  `$HOME/.local/state/pcloud-rs/publink-expiry.json`).
- macOS: `~/Library/Application Support/pcloud-rs/publink-expiry.json`.
- Windows: `%APPDATA%\pcloud-rs\publink-expiry.json`.

Example:

```toml
[plugins.publink_expiry]
enabled              = true
notify_window_hours  = 24
# state_file = "/home/alice/.local/state/pcloud-rs/publink-expiry.json"
```

Typical `notify_window_hours` values:

- `24`  — one-day heads-up (default).
- `72`  — long-weekend cushion for weekend-only operators.
- `168` — one-week cushion for enterprise workflows.

## 5. Lifecycle + event flow

```
 on_load ──▶ (no subscribe needed — plugin polls per tick)
                             │
                             ▼
           ┌────────────────────────────┐
           │ next_operation() cycle:    │
           │   1. TimerTick { 60 }      │   (informational)
           │   2. ObservePublinkList    │   (canonical query)
           └────────────┬───────────────┘
                        ▼
                   host replies
            PublinkList(Vec<PublinkSummary>)
                        │
                        ▼
        for each link with expiry_unix set:
            delta = expiry_unix - now
            if 0 ≤ delta ≤ notify_window_secs:
                if state.should_notify(link_id, now):
                    notifier.notify("Link expires", …)
                    state.mark_notified(link_id, now)
                        │
                        ▼
              atomic write state_file (0600)
```

## 6. Rule / event taxonomy

Each `PublinkSummary` is classified per tick:

| Condition                              | Action                                 |
|----------------------------------------|----------------------------------------|
| `expiry_unix == None`                  | Skip (link has no expiry).             |
| `delta > notify_window_secs`           | Skip (too far out).                    |
| `delta < 0`                            | Skip (already expired).                |
| `0 ≤ delta ≤ notify_window_secs`, `should_notify == true`  | Emit notification, persist state. |
| `0 ≤ delta ≤ notify_window_secs`, `should_notify == false` | Rate-limited (≤ 24h since last emit for this `link_id`). |

Rate limit: **one notification per `link_id` per 24h**, persisted
across restarts.

## 7. Outputs

- **Desktop notification** via `notify-rust` (libnotify on Linux,
  Notification Center on macOS, WinRT toast on Windows). Failures on
  headless hosts are swallowed; the plugin still updates its state.
- **State file**: a JSON map
  `{ "version": 1, "last_notified": { "<link_id>": <unix> } }` —
  written `0600`, atomic (`*.tmp` + rename), parent directory created
  on demand.
- **No IPC back to host** beyond the standard `next_operation` /
  `on_response` flow.
- **No audit log record** by default (the host may still record the
  observation op).

### Traits — for testability

- `Notifier` — abstracts the platform notifier. Production uses
  `DesktopNotifier`; tests use `CapturingNotifier`.
- `Clock` — abstracts wall time. Production uses `SystemClock`; tests
  inject `FixedClock`.

## 8. Test recipes

### Unit tests (built-in)

```bash
cargo test -p pcloud-plugin-publink-expiry
```

Covered: expiry-within-window emits notification; expiry-outside-window
does not; 24h rate-limit suppresses duplicate notifications; state file
round-trip; `0600` permissions on Unix; invalid `notify_window_hours`
rejected by `resolve_state_path`.

### Manual verification

```bash
export PCLOUD_PLUGINS_ENABLED=1

# 1. create a public link with a near-term expiry via the CLI
TOMORROW=$(date -d 'tomorrow' +%Y-%m-%d)
pcloudc create-link <path> --expires "$TOMORROW"

# 2. ensure the plugin is enabled
pcloudc config set plugins.publink_expiry.enabled true
pcloudc config set plugins.publink_expiry.notify_window_hours 48

# 3. kick the daemon so a tick occurs quickly, then wait ~60s
pcloudc list-public-links
sleep 60

# 4. you should see a desktop notification once
```

### Field-selector probe

```bash
# Are links visible to the plugin?
pcloudc --field link_id --json list-public-links
```

## 9. Failure modes

| Symptom                                        | Cause                                    | Remedy                                             |
|------------------------------------------------|------------------------------------------|----------------------------------------------------|
| No notification on a link that expires tomorrow | Headless host (no D-Bus)                 | Expected; plugin still records state.              |
| No notification on a link that expires tomorrow | `notify_window_hours` too small          | Raise to `48`+.                                    |
| Duplicate notifications                        | State file missing or unwritable         | Fix parent dir permissions; see `state_file`.      |
| `Initialization("...disabled...")` at startup  | `enabled = false` in config              | Set to `true`.                                     |
| `Config("notify_window_hours must be > 0")`    | Zero window                               | Set to `1` or more.                                |
| Plugin does not see new links until next tick  | Ticks are 60 s by default                 | Wait a minute; this is by design.                  |

## 10. Limitations (honest)

- **Advisory only.** Never auto-revokes, auto-renews, or rotates
  links. The remedy is `pcloudc change-link-expire` /
  `pcloudc delete-link`.
- **Headless hosts.** `notify-rust` needs a running notification
  daemon (e.g. D-Bus + libnotify on Linux). On servers or CI you will
  see no popups. There is no e-mail or webhook fallback by design —
  the plugin would need `NetworkEgress` for that and that capability
  has not been granted. Operators who want off-host notifications
  should pair this plugin with an external log-tail tool that watches
  the daemon's audit stream.
- **No enforcement.** The plugin warns; it does not renew, rotate, or
  delete links automatically. That is a deliberate scope limit —
  enforcement would require write operations against the public-link
  surface, which belong in an explicit CLI command, not a background
  plugin.
- **Clock skew.** Expiry is a server-reported UNIX timestamp; the
  plugin compares it against the host's local clock. Systems with
  significant clock drift may warn early or late. Use NTP.
- **Single-user scope.** The plugin observes the logged-in account
  only. Multi-account operation (if ever enabled) would register one
  instance per account.
- **State reset.** Deleting the state file resets per-link
  rate-limits. Worst case you see one extra notification per link —
  never silence or data loss.

## 11. Tuning: home deploy vs. FAANG enterprise

| Concern / knob          | Home / single-user            | Enterprise / fleet (single host still) |
|-------------------------|-------------------------------|----------------------------------------|
| `notify_window_hours`   | 24 (default)                   | 168 (weekly) or 336 (two weeks)        |
| `state_file` location   | XDG default                    | Explicit path under a monitored dir    |
| Headless fallback       | None — skip silently            | Pair with log-shipper that forwards the `ObservePublinkList` observation to central alerting |
| Multi-account           | Not applicable                 | Run one daemon instance per account    |
| Audit retention         | Local                          | Forward the host's audit stream        |

## 12. Security posture

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`.
- No `reqwest` / no network I/O inside the crate.
- No `SecretString` / `SecretBytes` crosses the plugin boundary;
  the host redacts the link list before delivery.
- The only durable artefact is the rate-limit JSON, which contains
  only opaque `link_id` strings and UNIX timestamps. Written `0600`;
  parent directory created on demand; writes are atomic (`*.tmp` +
  rename).
- Notification text is templated from non-secret fields; passwords,
  auth tokens, and short codes never appear in any visible surface.

## 13. Troubleshooting

- **No notifications on a headless host:** expected. See
  "Limitations".
- **"state file could not be read":** check that the parent
  directory is writable by the user running pcloud-rs and that
  `$XDG_STATE_HOME` (or `$HOME`) is set. Supply an explicit
  `state_file` path if needed.
- **Duplicate notifications after a reinstall:** deleting the state
  file resets the per-link rate limit. This is safe.
- **Plugin fails to load with `Initialization("...disabled...")`:**
  the config has `enabled = false`; set it to `true`.

## 14. CLI interactions

The plugin does not add any new `pcloudc` subcommands. It is entirely
observational, driven by daemon time-ticks. The CLI still participates
indirectly:

- `pcloudc list-links` / `pcloudc list-public-links` show the same
  set of links the plugin observes.
- `pcloudc change-link-expire <code> [YYYY-MM-DD]` is the user-facing
  remedy once a warning fires. Omit the date to clear the expiry
  outright.
- `pcloudc delete-link <code>` silences further warnings for that
  link.

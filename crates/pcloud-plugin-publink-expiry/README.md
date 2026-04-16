# pcloud-plugin-publink-expiry

Wave H7. First-party, single-user pcloud-rs plugin that raises a
desktop notification when a pCloud public link is within a
configurable window of expiring.

The authoritative user-facing documentation for this plugin lives in
[`docs/plugins/publink-expiry.md`](../../docs/plugins/publink-expiry.md).
This README is a short crate-level reference for developers hacking
on the plugin itself.

## Purpose

Operators routinely share pCloud public links with an expiry date set
and then forget about it. When a recipient reports a broken link the
first debugging question is always "did it just expire?". This
plugin closes the loop by observing the daemon's link list on every
scheduler tick and surfacing a local desktop notification before
expiry, so the user has a chance to refresh or re-issue the link
before it silently dies.

It is **advisory only**. It does not renew, rotate, or revoke links.

## Plugin-API ops introduced

- `PluginOperation::ObservePublinkList`
- `PluginOperation::TimerTick { period_secs }`

Both are defined in `pcloud-plugin-api`. The corresponding response
type is `PluginOperationResponse::PublinkList(Vec<PublinkSummary>)`,
where `PublinkSummary` is a redacted, non-secret view of a link (id,
kind, expiry UNIX timestamp) — no short URL, no password, no owner
id.

## Capabilities

| Capability        | Required |
|-------------------|:--------:|
| `ObserveStatus`   | yes      |
| `SyncControl`     | no       |
| `CryptoControl`   | no       |
| `NetworkEgress`   | no       |

No extra `PCLOUD_PLUGIN_ALLOW_*` flags are needed; the master
`PCLOUD_PLUGINS_ENABLED=1` is sufficient.

## Configuration knobs

`[plugins.publink_expiry]` in `pcloud.conf`:

| Key                    | Type   | Default            | Purpose                               |
|------------------------|--------|--------------------|---------------------------------------|
| `enabled`              | bool   | `true`             | Master switch for the plugin.         |
| `notify_window_hours`  | u32    | `24`               | How far before expiry to warn.        |
| `state_file`           | path   | XDG-derived        | Override rate-limit JSON location.    |

See the book chapter for platform-specific state-file defaults.

## Internal traits

- `Notifier` — abstracts the platform notifier (libnotify, macOS
  Notification Center, WinRT toast). Swapped for a mock in tests.
- `Clock` — abstracts wall time so the test suite can drive
  deterministic window checks (`ManualClock`).

## State file

A small JSON map (`link_id -> last_notified_unix`). Created `0600`
on Unix, parent directory created on demand, writes are atomic via
`*.tmp` + rename. Losing the file only resets per-link rate limits;
there is no sensitive material in it.

## Security posture

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`.
- No network I/O anywhere in the crate.
- No `SecretString` / `SecretBytes` crosses the plugin boundary.
- Only non-secret fields (`link_id`, `kind`, `expiry`) are ever
  observed; passwords and short-link URLs are redacted by the host
  before delivery.
- Notification text is built from non-secret fields only.

## Single-user scope

Operates on the currently logged-in account only. There is no
multi-tenant or fleet-wide mode. Multi-account operation, if ever
enabled upstream, would register one plugin instance per account.

## Honest limitations

- **Advisory only.** Never auto-revokes, auto-renews, or rotates
  links. The remedy is `pcloudc change-link-expire` /
  `pcloudc delete-link`.
- **Headless hosts.** No notifier → no popups. The plugin degrades
  silently; the integrity pipeline never stalls because of a missing
  notifier.
- **Clock skew.** Expiry is compared against the host's local clock.
  Use NTP.
- **State reset.** Deleting the state file costs at most one extra
  notification per link.

## Lifecycle (dev summary)

Per tick (60 s default):

1. `next_operation()` returns `TimerTick { period_secs: 60 }`
   (informational).
2. Then `ObservePublinkList`.
3. Host replies with `PublinkList(Vec<PublinkSummary>)`.
4. For each link with `0 ≤ expiry_unix - now ≤ notify_window_secs`
   and `should_notify == true`, the plugin emits a notification and
   persists the state file (0600, atomic).

## Internal trait seams

- `Notifier` — `DesktopNotifier` in production, `CapturingNotifier`
  in tests. Headless failures are swallowed.
- `Clock` — `SystemClock` / `FixedClock`. Tests use `FixedClock` for
  deterministic window checks.

## Tests

```bash
cargo test -p pcloud-plugin-publink-expiry
```

8 plugin-logic tests + 3 state-file integration tests. Covered:
expiry-within-window emits; expiry-outside-window does not;
24h rate-limit suppresses duplicates; state round-trip; 0600 on Unix.

### Manual smoke test

```bash
export PCLOUD_PLUGINS_ENABLED=1
TOMORROW=$(date -d 'tomorrow' +%Y-%m-%d)
pcloudc create-link <path> --expires "$TOMORROW"
pcloudc config set plugins.publink_expiry.notify_window_hours 48
# wait ~60 s for a tick; a single desktop notification should appear.
```

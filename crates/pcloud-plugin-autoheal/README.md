# pcloud-plugin-autoheal

Wave H8. First-party, single-user pcloud-rs plugin that reacts to
file-integrity mismatches emitted by the daemon's integrity scanner.

Authoritative user docs:
[`docs/plugins/autoheal.md`](../../docs/plugins/autoheal.md).

## Purpose

The integrity scanner on its own is a passive observer: it hashes
files, compares against recorded metadata, and writes the outcome to
the audit stream. Users who never read the audit log never learn
that a file has silently rotted. This plugin closes that loop:

1. Shows a rate-limited desktop notification for each `Mismatch`.
2. Asks the host to **quarantine** the affected sync root so the bad
   copy is not propagated further.
3. If the same path mismatches more than 3 times in 24h, escalates
   to a full `RequestSyncPause` on that root.

The plugin is **not a repair tool** — it never fetches a pristine
copy and overwrites the bad one. Repair is deliberately left to
explicit user action because automated rewrite on top of a
filesystem you cannot yet trust is the wrong default.

## Plugin-API ops introduced

- `PluginOperation::ObserveIntegrityEvents`
- `PluginOperation::RequestQuarantine { sync_root_id, path }`

Both defined in `pcloud-plugin-api`. Responses arrive via
`PluginOperationResponse::IntegrityEvent(FileIntegrityResult)`.

## Capabilities

| Capability        | Required |
|-------------------|:--------:|
| `ObserveStatus`   | yes      |
| `SyncControl`     | yes      |
| `CryptoControl`   | no       |
| `NetworkEgress`   | no       |

`SyncControl` requires `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1` in
addition to `PCLOUD_PLUGINS_ENABLED=1`. Both are off by default.

## Configuration knobs

`[plugins.autoheal]` in `pcloud.conf`:

| Key                               | Type | Default | Purpose                                   |
|-----------------------------------|------|---------|-------------------------------------------|
| `enabled`                         | bool | `true`  | Master switch.                            |
| `escalation_threshold_per_path`   | u32  | `3`     | Mismatches/24h before escalation.         |
| `max_quarantines_per_sync_root`   | u32  | `10`    | Quarantine requests/24h per sync root.    |
| `notification_cooldown_seconds`   | u32  | `3600`  | Per-path notification cooldown.           |

Escalations are additionally hard-capped at 1 per sync root per 24h
(not a config knob — this prevents runaway loops).

## Security posture

- `#![forbid(unsafe_code)]`, `#![deny(missing_docs)]`.
- Only non-secret data (`path`, `sync_root_id`, outcome enum)
  crosses the plugin boundary. `FileIntegrityResult` has no secret
  fields.
- No disk I/O. No network. No persistent state beyond the in-memory
  sliding window.
- Notification failures on headless hosts are swallowed — the
  integrity pipeline must not stall because there is no D-Bus.

## Single-user scope

Operates on the currently logged-in account's sync roots. No
cross-account reasoning. No fleet coordination.

## Honest limitations

- **Local escalation only.** The plugin pauses locally. It does
  **not** trigger an automatic resync against the server, does not
  contact a central control plane, and does not coordinate with
  other hosts.
- **Pauses are sticky.** The user must explicitly `pcloudc resume`.
  Silent auto-resume would hide an ongoing integrity problem.
- **Best-effort notifications.** Headless hosts get no popups;
  quarantine and pause actions still happen and are visible via
  `pcloudc status` and the audit stream.
- **Per-account scope only.** No multi-account logic.

## Lifecycle (dev summary)

- `on_load` emits `ObserveIntegrityEvents` once.
- Each `FileIntegrityResult::Mismatch` → rate-limited notify (1/path/hr)
  + `RequestQuarantine` (daily quota 10/root).
- `> 3` mismatches for the same path in 24h → `RequestSyncPause`
  (max 1/root/day).
- 24h sliding window; state pruned on every handled event.

## Internal trait seams

- `Clock` — abstracts wall time for deterministic tests.
- `Notifier` — abstracts desktop notification; tests use an in-memory
  capturing impl. Notification failures on headless hosts are
  swallowed.

## Tests

```bash
cargo test -p pcloud-plugin-autoheal
```

5 tests covering notify+quarantine, escalation threshold, daily
quarantine cap, `Ok` no-op, and notification rate-limit.

### Manual smoke test

```bash
export PCLOUD_PLUGINS_ENABLED=1
export PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1

# Corrupt a file post-hash, then trigger a rescan.
printf 'bitrot\n' >> ~/pcloud/docs/report.pdf
pcloudc sync localscan

# Observe autoheal events.
pcloudc --json audit verify | jq '.[] | select(.source == "pcloud-rs.autoheal")'
```

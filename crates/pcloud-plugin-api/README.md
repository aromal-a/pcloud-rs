# pcloud-plugin-api

Plugin manifest, ed25519 signature verification, capability taxonomy,
audit sink trait, and the `PluginRegistry` used by `pcloud-daemon` to
host first-party plugins.

Authoritative user-facing docs:
[`docs/plugins/README.md`](../../docs/plugins/README.md) and
the per-plugin pages it links to.

## What this crate provides

- `PluginManifest` — on-disk plugin manifest schema (id, version,
  display name, requested capabilities).
- `PluginSignature` — ed25519 detached signature over the manifest,
  verified before a plugin is ever loaded.
- `PluginCapability` — the four capability classes
  (`ObserveStatus`, `SyncControl`, `CryptoControl`, `NetworkEgress`)
  and the static mapping `PluginCapability::required_for(op)`.
- `PluginOperation` / `PluginOperationResponse` — the only typed
  boundary between a plugin and the daemon.
- `Plugin` — the trait a plugin crate implements.
- `PluginRegistry` — the daemon-side host that authorises ops and
  emits audit events.
- `PluginAuditSink` / `PluginAuditEvent` — structured audit trail for
  loads, denials, grants, and dispatch outcomes.
- `NullAuditSink` — drop-in sink for tests.

## PluginOperation surface

Full enumeration — the host understands exactly these variants and
refuses anything else:

| Variant                                                                | Required capability |
|------------------------------------------------------------------------|---------------------|
| `ObserveRuntimeSummary`                                                | `ObserveStatus`     |
| `ObserveHealth`                                                        | `ObserveStatus`     |
| `ObservePublinkList`                                                   | `ObserveStatus`     |
| `ObserveIntegrityEvents`                                               | `ObserveStatus`     |
| `TimerTick { period_secs }`                                            | `ObserveStatus`     |
| `PreUploadScan { path, size, content_hash, first_bytes, mime_guess }`  | `ObserveStatus`     |
| `RequestSyncPause { sync_root_id }`                                    | `SyncControl`       |
| `RequestSyncResume { sync_root_id }`                                   | `SyncControl`       |
| `RequestQuarantine { sync_root_id, path }`                             | `SyncControl`       |
| `QueryCryptoLockState`                                                 | `CryptoControl`     |
| `RequestNetworkProbe { host, port }`                                   | `NetworkEgress`     |

Responses live in `PluginOperationResponse` (`RuntimeSummary`,
`Health`, `PublinkList`, `IntegrityEvent`, `TimerAck`,
`CryptoLockState`, `NetworkProbe`, `UploadScanVerdict`).

Plugins only observe **narrow** data types at the boundary:

- `PublinkSummary` — `link_id`, kind, expiry timestamp. No short URL.
- `FileIntegrityResult` — path, sync root id, outcome enum. No content.
- `PreUploadScan` payload — `first_bytes` ≤ 4 KiB; `path` is available
  but DLP-class plugins must hash before logging.

## Capability taxonomy

Four classes, default-deny:

| Capability        | Env opt-in required?                    |
|-------------------|-----------------------------------------|
| `ObserveStatus`   | No (default-granted under `PCLOUD_PLUGINS_ENABLED`) |
| `SyncControl`     | `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1`    |
| `CryptoControl`   | `PCLOUD_PLUGIN_ALLOW_CRYPTO=1`          |
| `NetworkEgress`   | `PCLOUD_PLUGIN_ALLOW_NETWORK=1`         |

Granted set is `manifest ∩ ExtensionPolicy(env)`. Every dispatch is
authorised at runtime; denials emit
`PluginAuditEvent::InvocationDenied`.

## Usage (host side)

```rust,no_run
use pcloud_plugin_api::{PluginRegistry, NullAuditSink};

let mut registry = PluginRegistry::new();
let mut audit = NullAuditSink;

// Typical daemon bootstrap: build extension policy from env,
// register vetted plugins, then authorise each op before dispatch.
# drop((registry, audit));
```

## Usage (plugin side)

See
[Adding a Plugin](../../docs/book/src/development/adding-a-plugin.md)
for a step-by-step walkthrough, or any of the four first-party
crates:

- `pcloud-plugin-publink-expiry`
- `pcloud-plugin-autoheal`
- `pcloud-plugin-backup-schedule`
- `pcloud-plugin-dlp`

## Features

None.

## Security posture

- Signature verification is mandatory and cannot be disabled at
  runtime.
- `sha2` + `ed25519-dalek` are used in their std configurations.
- `PluginContext` is deliberately narrow — a compile-time proof in
  the test suite enforces that it exposes only
  `granted_capabilities` and `runtime_hint`. Adding a secret-bearing
  field would fail CI.
- Operations are the only boundary. There is no raw function pointer,
  no FFI, no dynamic loader.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

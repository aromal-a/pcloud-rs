# pcloud-rs Plugins

This directory is the entry point for anyone writing a plugin against the
pcloud-rs Rust rewrite, or operating an install that has one of the
first-party plugins enabled. For a step-by-step walkthrough of creating
a new plugin crate, see
[Adding a Plugin](../book/src/development/adding-a-plugin.md) in the
book.

All first-party plugins live in the workspace under
`crates/pcloud-plugin-*` and are **statically linked** into the
daemon binary. There is no dynamic `.so`/`.dll` loader. Enabling or
disabling a plugin is a flag in the daemon configuration, not a file
dropped into a directory.

## Catalogue

| Plugin                          | Wave | Purpose                                                          | Capabilities                    | Doc |
|---------------------------------|-----:|------------------------------------------------------------------|---------------------------------|-----|
| `pcloud-plugin-publink-expiry`  | H7   | Desktop-notify when a public link is about to expire             | `ObserveStatus`                 | [publink-expiry.md](./publink-expiry.md) |
| `pcloud-plugin-autoheal`        | H8   | React to file-integrity mismatches: notify, quarantine, pause    | `ObserveStatus`, `SyncControl`  | [autoheal.md](./autoheal.md) |
| `pcloud-plugin-backup-schedule` | H9   | In-process cron that resumes sync roots on a schedule            | `SyncControl`                   | [backup-schedule.md](./backup-schedule.md) |
| `pcloud-plugin-dlp`             | H10  | Pre-upload scanner for obvious secret material (keys, PEM, JWT)  | `ObserveStatus`                 | [dlp-builtin.md](./dlp-builtin.md) |
| `pcloud-plugin-dlp-enterprise`  |  —   | Enterprise-tier DLP (custom rulesets, audit streaming)           | `ObserveStatus`                 | (see `enterprise/` docs) |

Additional plugins planned for future waves will be added to this table
before they land in `main`.

### Honesty callouts (apply to every plugin below)

- **Pre-alpha.** All four plugins are first-party *but* first-release
  quality. Expect edge cases, especially around headless hosts and
  clock jumps.
- **Single-user scope.** Every plugin operates on the currently
  logged-in account's sync roots. There is no multi-tenant mode, no
  fleet coordination, no central policy server.
- **Advisory only.** No plugin auto-revokes a link, auto-repairs a
  file, auto-deletes content, or auto-enforces anything on its own. The
  host enforces; the plugin decides.
- **Off by default.** The plugin runtime itself requires
  `PCLOUD_PLUGINS_ENABLED=1` *and* a per-plugin `enabled = true` in
  `pcloud.conf`. Sensitive capability classes require a second env
  opt-in (`PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL`, etc.).

## Plugins vs. enterprise features

`pcloud-rs` has two extension mechanisms, and they exist for different
audiences.

|                           | Plugins (`pcloud-plugin-*`)                      | Enterprise features (`pcloud-enterprise-*`)   |
|---------------------------|--------------------------------------------------|-----------------------------------------------|
| Deployment model          | Single-user, on the same host as the daemon      | Fleet-wide, integrated with external systems  |
| Linkage                   | Static at build time                             | Static at build time, gated by crate features |
| Secret handling           | Host redacts at the boundary; plugin never sees  | May integrate with KMS, OIDC, Vault           |
| Enable path               | `pcloud.conf` + env opt-in                       | Build feature + config + org policy           |
| Typical reviewer          | End user / home lab                              | Security engineer / compliance                |

The `pcloud-plugin-dlp` plugin and the `pcloud-enterprise-dlp` feature
illustrate the split: the plugin is a small regex scanner for local
mistakes, the enterprise feature integrates with a central DLP service.
They share the pre-upload boundary type but nothing else.

## Plugin runtime model

### Registry and manifest

Every plugin provides a `PluginManifest` declaring:

- a stable `id` (e.g. `pcloud-rs.publink-expiry`),
- a `display_name`,
- a `version` (usually `env!("CARGO_PKG_VERSION")`),
- a `BTreeSet<PluginCapability>` of requested capabilities.

The daemon's `PluginRegistry` (see
`crates/pcloud-plugin-api/src/lib.rs`) validates the manifest at
registration time, checks the ed25519 signature on any externally
distributed plugin (n/a for first-party static crates), and emits a
`PluginAuditEvent::LoadAccepted` or `LoadRejected` event.

### Lifecycle

```
 ┌──────────┐   register    ┌──────────┐   on_load    ┌──────────┐
 │ Manifest │──────────────▶│ Registry │─────────────▶│  Plugin  │
 └──────────┘               └──────────┘              └────┬─────┘
                                                           │ next_operation()
                                       ObserveStatus etc.  ▼
                                                   ┌──────────────┐
                                          ◀───────▶│   Runtime    │
                                 PluginOperation   │  (dispatcher)│
                                                   └──────┬───────┘
                                                          │
                                         PluginOperationResponse
                                                          ▼
                                                   ┌──────────────┐
                                                   │ on_response  │
                                                   └──────┬───────┘
                                                          │  shutdown
                                                          ▼
                                                   ┌──────────────┐
                                                   │    Drop      │
                                                   └──────────────┘
```

Methods (all on the `Plugin` trait):

- `manifest(&self) -> PluginManifest` — pure, called before load.
- `on_load(&mut self, ctx: &PluginContext)` — redacted context, one-shot.
- `next_operation(&mut self) -> Option<PluginOperation>` — polled; return
  `None` to idle.
- `on_response(&mut self, r: &PluginOperationResponse)` — called per reply.
- Drop runs on `shutdown` — no explicit shutdown hook today.

### PluginOperation surface

All operations the host currently understands (enum
`pcloud_plugin_api::PluginOperation`):

| Variant                                        | Purpose                                                 | Required capability |
|------------------------------------------------|---------------------------------------------------------|---------------------|
| `ObserveRuntimeSummary`                        | One-shot daemon status blob                             | `ObserveStatus`     |
| `ObserveHealth`                                | Healthcheck result                                      | `ObserveStatus`     |
| `ObservePublinkList`                           | Redacted `PublinkSummary` list                          | `ObserveStatus`     |
| `ObserveIntegrityEvents`                       | Subscribe to `FileIntegrityResult` stream               | `ObserveStatus`     |
| `TimerTick { period_secs }`                    | Request a recurring tick (informational)                | `ObserveStatus`     |
| `PreUploadScan { path, size, content_hash, first_bytes, mime_guess }` | Synchronous pre-upload scan           | `ObserveStatus`     |
| `RequestSyncPause { sync_root_id }`            | Ask host to pause a sync root                           | `SyncControl`       |
| `RequestSyncResume { sync_root_id }`           | Ask host to resume a sync root                          | `SyncControl`       |
| `RequestQuarantine { sync_root_id, path }`     | Ask host to quarantine a specific path                  | `SyncControl`       |
| `QueryCryptoLockState`                         | Query whether crypto is locked                          | `CryptoControl`     |
| `RequestNetworkProbe { host, port }`           | Ask host to probe a remote endpoint (never the plugin)  | `NetworkEgress`     |

Corresponding responses (`PluginOperationResponse`):

- `RuntimeSummary(…)` / `Health(…)` / `PublinkList(Vec<PublinkSummary>)`
- `IntegrityEvent(FileIntegrityResult)` (one per event after subscribe)
- `TimerAck` (acknowledgement for a `TimerTick`)
- `CryptoLockState(bool)` / `NetworkProbe(ProbeOutcome)`
- `UploadScanVerdict(UploadScanVerdict)` — `Allow`, `Deny`,
  `Quarantine`, or `RedactAndAllow`.

`PublinkSummary`, `FileIntegrityResult`, and the `PreUploadScan`
payload are **narrow by design** — no auth tokens, no passwords, no
short-link URLs, no raw file contents beyond the `first_bytes` window
(typically ≤ 4 KiB). The registry will not widen them.

## Capabilities — default-deny semantics

Four capability classes exist:

| Capability        | Unlocks                                                | Env opt-in                          |
|-------------------|--------------------------------------------------------|-------------------------------------|
| `ObserveStatus`   | Status / health / publink list / integrity / timer / pre-upload scan | *(granted by default when plugins enabled)* |
| `SyncControl`     | `RequestSyncPause`, `RequestSyncResume`, `RequestQuarantine`          | `PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1` |
| `CryptoControl`   | `QueryCryptoLockState` (no key material is ever returned)            | `PCLOUD_PLUGIN_ALLOW_CRYPTO=1`       |
| `NetworkEgress`   | `RequestNetworkProbe` (probe is executed by host, not plugin)        | `PCLOUD_PLUGIN_ALLOW_NETWORK=1`      |

Rules:

- The registry **default-denies** any operation outside the declared
  manifest set.
- Capability classes are AND-ed with the environment policy. Requesting
  a capability in a manifest does not grant it — the operator must
  also flip the matching env flag.
- At dispatch, every op is authorised against the granted set; denials
  emit a `PluginAuditEvent::InvocationDenied` and return
  `PluginError::CapabilityNotGranted`.

### Capability enforcement is **runtime-gated**

Capability checks are not an honour system enforced by individual
callers. They are enforced inside the registry itself, at the single
choke point `PluginRegistry::dispatch` (see
`crates/pcloud-plugin-api/src/lib.rs`). Every host dispatcher — the
DLP pre-upload hook, the autoheal quarantine path, the backup
scheduler, the publink-expiry notifier — **must** run plugin work
through `dispatch`, which:

1. Calls `PluginCapability::required_for(op)` and compares against the
   plugin's granted set. A missing capability means the handler is
   never invoked; the registry emits a structured
   `PluginAuditEvent::InvocationDenied` audit event of the shape
   `plugin.capability.denied{plugin, op, missing}` and returns
   `PluginError::CapabilityNotGranted`.
2. Runs the handler inside `std::panic::catch_unwind`. A panicking
   plugin cannot crash the daemon: the registry swallows the panic
   payload (it is **not** forwarded to the audit sink — the payload may
   contain plugin-constructed data), emits
   `PluginAuditEvent::HandlerPanic` + `PluginAuditEvent::PluginDeregistered`,
   and **de-registers** the offending plugin. All subsequent calls to
   that plugin id return `PluginError::UnknownPlugin`.
3. Never exposes secrets: even on the deny path, the audit record is a
   fixed-shape structured event — never a raw capability grant or a
   transcript of what the plugin was trying to do.

In short: a plugin that lost a capability (via operator config or env
revocation) cannot perform the gated operation even if it tries to call
a host API directly, because the only path to the host API goes through
`PluginRegistry::dispatch`.

## Trust model — read before enabling anything

The plugin host is **not a sandbox**. Plugins are:

- linked into the daemon at build time,
- run in-process, in the daemon's address space,
- bound by the same OS privileges as `pcloudd` itself,
- free (in principle) to panic the daemon if they contain bugs.

What the host **does** enforce:

- **Capability grants.** See above.
- **Data redaction at the boundary.** Types crossing the plugin
  boundary (`PublinkSummary`, `FileIntegrityResult`,
  `PreUploadScanRequest`) are deliberately narrow and never contain
  auth tokens, passwords, crypto keys, short-link URLs, or raw file
  contents outside the pre-upload window.
- **Environment-level opt-in.** The runtime is off by default; sensitive
  capability classes require a second opt-in.

What the host does **not** do:

- It does **not** run plugins in a separate process, WASM runtime, or
  seccomp jail. A malicious plugin is exactly as dangerous as any other
  code compiled into the daemon.
- It does **not** verify third-party signatures. Only plugins shipped
  in-tree as part of this workspace are trusted by default.

Rule of thumb: treat adding a plugin like accepting a dependency into
the daemon's own `Cargo.toml`. If you would not merge the code, do not
enable it.

## Cross-plugin security posture

| Property                                     | publink-expiry | autoheal | backup-schedule | dlp  |
|----------------------------------------------|:--------------:|:--------:|:---------------:|:----:|
| `#![forbid(unsafe_code)]`                    | yes            | yes      | yes             | yes  |
| `#![deny(missing_docs)]`                     | yes            | yes      | yes             | yes  |
| Zero network I/O from the plugin             | yes            | yes      | yes             | yes  |
| Zero disk I/O from the plugin                | *(state only)* | yes      | yes             | yes  |
| Never logs a raw file path                   | yes            | yes      | yes             | yes (SHA-256 hash only) |
| Never logs any file contents                 | yes            | yes      | yes             | yes  |
| Rate-limited user-visible notifications      | 1 / link / 24h | 1 / path / 1h | n/a        | n/a  |

DLP enforces the **strongest** redaction discipline: audit records
contain only the SHA-256 hex of the file path, the matched rule IDs,
and the verdict. No `first_bytes`, no regex match text, and no file
paths ever reach the audit log.

## How to enable a plugin

1. Enable the plugin runtime itself:

   ```bash
   export PCLOUD_PLUGINS_ENABLED=1
   ```

   If a plugin needs a sensitive capability class, also opt into that
   class (`PCLOUD_PLUGIN_ALLOW_SYNC_CONTROL=1` for autoheal and
   backup-schedule, etc.). Capability switches are AND-ed with the
   per-plugin manifest.

2. Turn the plugin on in your config file under its `[plugins.*]`
   section:

   ```toml
   [plugins.publink_expiry]
   enabled = true

   [plugins.autoheal]
   enabled = true

   [plugins.backup_schedule]
   enabled = true

   [plugins.dlp]
   enabled     = true
   strict_mode = false    # audit-only until you are ready to enforce
   ```

3. Restart the daemon (`pcloudc stop && pcloudc start`). Because
   plugins are statically linked, there is nothing to load or unload
   at runtime; the registry is built once during daemon bootstrap.

## Where configuration lives

Plugin config lives inside the main daemon config file
(`pcloud.conf`), under the `[plugins.*]` namespace. The search path
for that file is the standard one documented in `pcloud.conf(5)`:

- Linux: `$XDG_CONFIG_HOME/pcloud-rs/pcloud.conf`
- macOS: `~/Library/Application Support/pcloud-rs/pcloud.conf`
- Windows: `%APPDATA%\pcloud-rs\pcloud.conf`

Any durable state a plugin keeps (e.g. the `publink-expiry` rate-limit
JSON) lives under `$XDG_STATE_HOME/pcloud-rs/` on Linux (with
OS-appropriate equivalents elsewhere). Plugins may override their state
file via config; see the per-plugin pages.

## Writing your own plugin

The public API lives in `pcloud-plugin-api`. Every plugin implements the
`Plugin` trait, declares its capabilities in a manifest, and is
registered from the daemon bootstrap. Until the static-only policy
changes, any new plugin must be added to this workspace and compiled
into the daemon build.

See the detailed walk-through in
[Adding a Plugin](../book/src/development/adding-a-plugin.md).

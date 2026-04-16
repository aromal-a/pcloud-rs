# Adding a Plugin

This chapter walks a contributor through adding a new first-party plugin to
the pcloud-rs Rust workspace end-to-end. Plugins are **statically linked**
into the daemon binary — there is no dynamic `.so` / `.dll` loader, and a
new plugin is always a new crate inside `crates/`.

If you are hacking on one of the four existing plugins, see its own page
in [Plugins](../../../plugins/README.md) first:

- [Publink Expiry Notifier](../../../plugins/publink-expiry.md)
- [Auto-Heal Checksum Scanner](../../../plugins/autoheal.md)
- [Backup Schedule](../../../plugins/backup-schedule.md)
- [Built-in DLP Pre-Upload Scanner](../../../plugins/dlp-builtin.md)

A new plugin touches **seven layers**:

1. A new crate under `crates/pcloud-plugin-<name>/`.
2. A `Plugin` trait implementation on top of `pcloud-plugin-api`.
3. A `PluginManifest` declaring the capabilities you want.
4. Host registration from the daemon bootstrap path.
5. Config schema (`[plugins.<name>]`) parsed by `pcloud-config`.
6. Tests: unit + an integration test that registers and drives the plugin.
7. User-facing docs under `docs/plugins/<name>.md` and a crate
   `README.md` under `crates/pcloud-plugin-<name>/`.

## 1. Decide what the plugin actually needs

Before writing code, pick exactly which `PluginOperation` variants your
plugin will emit. The runtime refuses any op outside the set the manifest
requested. The full enumeration is defined in
[`pcloud-plugin-api`](../../../../crates/pcloud-plugin-api/src/lib.rs); the
short summary is in the [plugins overview](../../../plugins/README.md#pluginoperation-surface).

Capability matrix:

| Operation                      | Required capability |
|--------------------------------|---------------------|
| `ObserveRuntimeSummary`        | `ObserveStatus`     |
| `ObserveHealth`                | `ObserveStatus`     |
| `ObservePublinkList`           | `ObserveStatus`     |
| `ObserveIntegrityEvents`       | `ObserveStatus`     |
| `TimerTick { period_secs }`    | `ObserveStatus`     |
| `PreUploadScan { … }`          | `ObserveStatus`     |
| `RequestSyncPause { … }`       | `SyncControl`       |
| `RequestSyncResume { … }`      | `SyncControl`       |
| `RequestQuarantine { … }`      | `SyncControl`       |
| `QueryCryptoLockState`         | `CryptoControl`     |
| `RequestNetworkProbe { … }`    | `NetworkEgress`     |

The registry default-denies anything the manifest did not request. Keep
the list minimal.

## 2. Create the crate

```bash
cd crates
cargo new --lib pcloud-plugin-<name>
```

In `Cargo.toml`, add the plugin API as a path dependency:

```toml
[dependencies]
pcloud-plugin-api = { path = "../pcloud-plugin-api" }
serde = { version = "1", features = ["derive"] }
thiserror = "2"
```

Add the crate to the workspace `members` list in `Cargo.toml`.

## 3. Implement `Plugin`

A skeleton:

```rust
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use pcloud_plugin_api::{
    Plugin, PluginCapability, PluginContext, PluginError,
    PluginManifest, PluginOperation, PluginOperationResponse,
};
use std::collections::{BTreeSet, VecDeque};

/// Example placeholder plugin.
pub struct MyPlugin {
    pending: VecDeque<PluginOperation>,
}

impl MyPlugin {
    /// Construct a new instance.
    pub fn new() -> Self {
        Self { pending: VecDeque::new() }
    }
}

impl Plugin for MyPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "pcloud-rs.my-plugin".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            display_name: "My Plugin".to_owned(),
            requested_capabilities: BTreeSet::from([
                PluginCapability::ObserveStatus,
            ]),
        }
    }

    fn on_load(&mut self, _ctx: &PluginContext) -> Result<(), PluginError> {
        self.pending.push_back(PluginOperation::ObserveRuntimeSummary);
        Ok(())
    }

    fn next_operation(&mut self) -> Option<PluginOperation> {
        self.pending.pop_front()
    }

    fn on_response(&mut self, _r: &PluginOperationResponse) { /* … */ }
}
```

Rules that the registry enforces for you:

- `on_load` sees only a redacted `PluginContext` — no secrets, no
  account identifiers, no token handles.
- `next_operation` is polled repeatedly; return `None` when idle.
- `on_response` is called with the host's reply to your op. Only ops
  in your manifest will ever reach the host.
- `CapabilityDenied` / `CapabilityNotGranted` errors are raised if you
  emit an op you did not declare.

## 4. Wire the plugin into daemon bootstrap

Register the plugin in `pcloud-daemon` bootstrap, behind the existing
per-plugin config toggle:

```rust
// crates/pcloud-daemon/src/bootstrap.rs
if config.plugins.my_plugin.enabled {
    registry.register(Box::new(pcloud_plugin_my_plugin::MyPlugin::new()))?;
}
```

Any sensitive capability (`SyncControl`, `CryptoControl`,
`NetworkEgress`) must be also opted in at the environment level with the
corresponding `PCLOUD_PLUGIN_ALLOW_*` flag, on top of the master
`PCLOUD_PLUGINS_ENABLED=1`. Capability grants are computed as:

    granted = manifest ∩ ExtensionPolicy(env flags)

## 5. Add config schema

In `pcloud-config`, extend the top-level `PluginsConfig` with your
section:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MyPluginConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    // … fields …
}
```

Document every key in your plugin's user-facing doc under
`docs/plugins/<name>.md`. Unlisted keys default to the struct's
`Default` impl.

## 6. Test

Write three kinds of tests:

1. **Unit tests** inside `src/lib.rs` under `#[cfg(test)] mod tests`.
2. **Integration tests** under `crates/pcloud-plugin-<name>/tests/` that
   register the plugin with a real `PluginRegistry` and assert the
   ops and responses go through.
3. **Host smoke test**: extend `crates/pcloud-daemon/tests/` to include
   the new plugin under a feature flag or config toggle.

Run focused tests with:

```bash
cargo test -p pcloud-plugin-<name>
```

Any plugin that uses wall-clock time must be deterministic under a
test `Clock` injection (see `publink-expiry` / `backup-schedule` for
the trait pattern).

## 7. Document

Every first-party plugin in this workspace has:

- A crate-local `README.md` (developer-facing, pointer to the book page).
- A book page at `docs/plugins/<name>.md` (user-facing).
- An entry in `docs/plugins/README.md`'s catalogue table.
- An entry in `docs/book/src/SUMMARY.md` under the Plugins section.

Do not skip the honesty callouts. Every current plugin says, in its
"Limitations" section: *single-user, advisory only, off by default*. If
your new plugin has the same scope, say so in the same words.

## Checklist

- [ ] Crate compiles: `cargo build -p pcloud-plugin-<name>`
- [ ] `#![forbid(unsafe_code)]` + `#![deny(missing_docs)]` at crate root
- [ ] Manifest declares only the capabilities the plugin uses
- [ ] Host bootstrap registers the plugin behind `config.plugins.<name>.enabled`
- [ ] Config schema has safe defaults
- [ ] Unit + integration tests pass: `cargo test -p pcloud-plugin-<name>`
- [ ] `docs/plugins/<name>.md` created and linked from `SUMMARY.md`
- [ ] Catalogue row added to `docs/plugins/README.md`
- [ ] `C_FEATURE_PARITY_MATRIX.csv` unchanged (plugins are additive)

If any of the above is not true, the plugin is not ready to merge.

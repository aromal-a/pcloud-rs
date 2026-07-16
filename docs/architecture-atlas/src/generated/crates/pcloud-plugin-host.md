# `pcloud-plugin-host`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-host`

**Manifest:** [`crates/pcloud-plugin-host/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/Cargo.toml)

Sandboxed plugin host scaffold (T2.5). Capability-bound message-bus model; wasmtime backing tracked for follow-up integration.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_host` | lib | [`crates/pcloud-plugin-host/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs) |

## Direct dependencies

`serde`, `serde_json`, `thiserror`

## Cargo features

No declared package features.

## File inventory (2)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-host/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/Cargo.toml) | Cargo manifest | Defines package/workspace metadata, features, targets, and dependencies. |
| [`crates/pcloud-plugin-host/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs) | library root | T2.5 — sandboxed plugin host scaffold. |

## Rust declaration index (40 total; 23 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `Capability` | `pub` | enum | [`crates/pcloud-plugin-host/src/lib.rs:61`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L61) | Capability the host can grant to a plugin. Granting is explicit (the absence of a capability denies the corre… |
| `PluginId` | `pub` | struct | [`crates/pcloud-plugin-host/src/lib.rs:76`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L76) | Plugin identity. The host uses this to bind the capability allowlist to the loaded module so a plugin cannot… |
| `new` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:86`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L86) | Construct a plugin id from a string. Empty / whitespace- only ids are rejected. # Errors Returns \[`HostError:… |
| `as_str` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:96`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L96) | String view of the id. |
| `CapabilitySet` | `pub` | struct | [`crates/pcloud-plugin-host/src/lib.rs:103`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L103) | Compiled capability set bound to one plugin instance. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:110`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L110) | Empty set (no capabilities; every host call denies). |
| `from_capabilities` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:116`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L116) | Build a set from an iterator of capabilities. |
| `grant` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:123`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L123) | Grant `cap`. |
| `revoke` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:128`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L128) | Revoke `cap`. No-op if not granted. |
| `allows` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:134`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L134) | `true` when `cap` has been granted. |
| `granted` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:140`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L140) | Sorted list of granted capabilities (for audit / diagnostics). |
| `HostRequest` | `pub` | enum | [`crates/pcloud-plugin-host/src/lib.rs:149`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L149) | One message the plugin sends to the host. Every variant maps to a single \[`Capability`\] check enforced by `Ho… |
| `required_capability` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:174`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L174) | Capability required to dispatch this request. |
| `HostResponse` | `pub` | enum | [`crates/pcloud-plugin-host/src/lib.rs:187`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L187) | One message the host returns to the plugin. |
| `HostError` | `pub` | enum | [`crates/pcloud-plugin-host/src/lib.rs:216`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L216) | Errors raised by the plugin host. |
| `PluginBackend` | `pub` | trait | [`crates/pcloud-plugin-host/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L239) | Trait the host uses to drive a concrete plugin runtime. `NoopBackend` is the default implementation (proves t… |
| `load` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:243`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L243) | Load a wasm module / native plugin and return a handle. Returns \[`HostError::LoadFailed`\] if the bytes are in… |
| `deliver` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:248`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L248) | Hand the plugin one host response. Plugins consume these asynchronously; the host calls this for each pending… |
| `NoopBackend` | `pub` | struct | [`crates/pcloud-plugin-host/src/lib.rs:254`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L254) | No-op backend: accepts any module, accepts any delivery. Used by tests and as the default until wasmtime land… |
| `load` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:257`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L257) | Read the source/rustdoc for the exact contract. |
| `deliver` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:260`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L260) | Read the source/rustdoc for the exact contract. |
| `HostBus` | `pub` | struct | [`crates/pcloud-plugin-host/src/lib.rs:272`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L272) | Host-side message bus. Owns the capability-set table and dispatches plugin requests through the configured ba… |
| `new` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:279`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L279) | Empty bus. |
| `register` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:285`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L285) | Register a plugin with its capability set. Replaces any existing entry for the same id. |
| `deregister` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:291`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L291) | Drop a plugin's capability set. |
| `capabilities_of` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:298`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L298) | Return the capability set for `plugin_id`. Empty set when the plugin is not registered. |
| `authorise` | `pub` | fn | [`crates/pcloud-plugin-host/src/lib.rs:314`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L314) | Authorise a request against `plugin_id`'s capability set. The host should call this before performing any act… |
| `tests` | `private` | mod | [`crates/pcloud-plugin-host/src/lib.rs:328`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L328) | Read the source/rustdoc for the exact contract. |
| `pid` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:331`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L331) | Read the source/rustdoc for the exact contract. |
| `plugin_id_rejects_empty` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:336`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L336) | Read the source/rustdoc for the exact contract. |
| `capability_set_grant_revoke` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:345`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L345) | Read the source/rustdoc for the exact contract. |
| `host_request_maps_to_required_capability` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:355`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L355) | Read the source/rustdoc for the exact contract. |
| `unregistered_plugin_is_denied` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:370`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L370) | Read the source/rustdoc for the exact contract. |
| `registered_plugin_with_cap_passes_authorise` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:385`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L385) | Read the source/rustdoc for the exact contract. |
| `registered_plugin_without_cap_is_denied` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:396`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L396) | Read the source/rustdoc for the exact contract. |
| `audit_log_denied_without_capability` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:417`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L417) | Acceptance pivot: a plugin without the audit-log capability cannot enqueue an event. |
| `deregister_drops_capabilities` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:438`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L438) | Read the source/rustdoc for the exact contract. |
| `noop_backend_round_trips` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:449`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L449) | Read the source/rustdoc for the exact contract. |
| `capabilities_serde_roundtrip` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:464`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L464) | Read the source/rustdoc for the exact contract. |
| `host_request_serde_roundtrip` | `private` | fn | [`crates/pcloud-plugin-host/src/lib.rs:475`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-host/src/lib.rs#L475) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

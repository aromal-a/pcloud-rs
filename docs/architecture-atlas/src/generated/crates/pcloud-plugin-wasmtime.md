# `pcloud-plugin-wasmtime`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-plugin-wasmtime`

**Manifest:** [`crates/pcloud-plugin-wasmtime/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/Cargo.toml)

T2.5.b — wasmtime-backed `PluginBackend` impl for `pcloud-plugin-host`. Kept in a separate crate so the host core stays dep-light.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_plugin_wasmtime` | lib | [`crates/pcloud-plugin-wasmtime/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs) |

## Direct dependencies

`log`, `pcloud-plugin-host`, `thiserror`, `wasmtime`

## Cargo features

No declared package features.

## File inventory (2)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-plugin-wasmtime/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/Cargo.toml) | Cargo manifest | Wasmtime 43 fixed the sandbox advisories cited below and requires Rust 1.91. |
| [`crates/pcloud-plugin-wasmtime/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs) | library root | T2.5.b — wasmtime-backed `PluginBackend` for `pcloud-plugin-host`. |

## Rust declaration index (19 total; 4 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `WasmtimeBackendError` | `pub` | enum | [`crates/pcloud-plugin-wasmtime/src/lib.rs:46`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L46) | Errors raised by the wasmtime backend before they are translated into \[`HostError`\]. |
| `from` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:64`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L64) | Read the source/rustdoc for the exact contract. |
| `WasmtimeBackend` | `pub` | struct | [`crates/pcloud-plugin-wasmtime/src/lib.rs:73`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L73) | Wasmtime-backed plugin runtime. Kept deliberately minimal: the engine has WASI **disabled**, so any plugin th… |
| `fmt` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:79`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L79) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:94`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L94) | Construct a new backend with a default `wasmtime::Engine`. # Errors Returns \[`WasmtimeBackendError::Engine`\]… |
| `loaded_count` | `pub` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:104`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L104) | How many modules are currently loaded. Diagnostics-only. |
| `validate_no_imports` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:117`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L117) | Validate a module by attempting to instantiate it against an **empty** linker. This is the heart of the sandb… |
| `load` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:128`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L128) | Read the source/rustdoc for the exact contract. |
| `deliver` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:142`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L142) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-plugin-wasmtime/src/lib.rs:154`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L154) | Read the source/rustdoc for the exact contract. |
| `pid` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:158`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L158) | Read the source/rustdoc for the exact contract. |
| `EMPTY_WASM` | `private` | const | [`crates/pcloud-plugin-wasmtime/src/lib.rs:165`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L165) | Minimal valid wasm module: 4-byte magic + 4-byte version, no sections. wasmtime accepts this and there are no… |
| `fd_write_import_wasm` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:178`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L178) | Hand-written wasm module that imports `wasi_snapshot_preview1.fd_write`. Layout: magic(4) + version(4) type s… |
| `wasmtime_backend_constructs` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:214`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L214) | Read the source/rustdoc for the exact contract. |
| `wasmtime_module_with_no_imports_loads_ok` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:222`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L222) | Acceptance: a no-import sample module loads cleanly, confirming the sandbox is not pathologically over-strict. |
| `wasmtime_module_with_fs_import_fails_to_load` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:233`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L233) | Deny-path acceptance pivot. A module that imports `wasi_snapshot_preview1.fd_write` (the wasm equivalent of `… |
| `deliver_unknown_plugin_errors` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:261`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L261) | Read the source/rustdoc for the exact contract. |
| `deliver_known_plugin_ok` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:276`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L276) | Read the source/rustdoc for the exact contract. |
| `invalid_bytes_rejected` | `private` | fn | [`crates/pcloud-plugin-wasmtime/src/lib.rs:283`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-plugin-wasmtime/src/lib.rs#L283) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

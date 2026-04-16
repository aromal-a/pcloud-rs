# pcloud-config

Typed configuration loader and validator for the pcloud-rs Rust workspace.

## What this crate does

- Loads daemon and CLI configuration from JSON files and environment variables.
- Validates transport policy (TLS required in production), data-dir
  permissions, and endpoint overrides before returning to callers.
- Is the single source of truth for paths such as the runtime socket,
  vault directory, and SQLite store.

## Public API entry points

- `Config::load`, `Config::from_env`, `Config::validate`.
- Typed enums for `TransportPolicy`, `ApiEndpoint`, and related options.

## Usage

```rust,no_run
use pcloud_config::Config;

let cfg = Config::load_default()?;
assert!(cfg.transport_requires_tls());
# Ok::<(), pcloud_config::Error>(())
```

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

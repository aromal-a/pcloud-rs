# pcloud-error

Shared error types and result aliases for the pcloud-rs Rust workspace.

## What this crate does

- Provides a single root `Error` enum (via `thiserror`) and a `Result<T>` alias.
- Lets every other crate map its domain errors into a consistent top-level
  surface for CLI, SDK, and daemon callers.

## Public API entry points

- `Error`, `Result`, and conversion helpers.

## Usage

```rust
use pcloud_error::{Error, Result};

fn check() -> Result<()> {
    Ok(())
}
```

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

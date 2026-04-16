# pcloud-cache

In-memory caching primitives for pcloud-rs metadata and transfer state.

## What this crate does

- Provides lightweight, thread-safe caches for folder listings, file metadata,
  and short-lived transfer handles.
- Has no I/O and no persistence: durable storage lives in `pcloud-store`.

## Public API entry points

- `MetadataCache` and helpers for insert/get/invalidate.
- Cache-key newtypes that wrap `pcloud-model` identifiers.

## Usage

```rust,no_run
use pcloud_cache::MetadataCache;

let cache = MetadataCache::new();
let _ = cache.len();
```

## Features

None. Pure-Rust, no optional features.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

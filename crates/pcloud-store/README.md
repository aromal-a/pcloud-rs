# pcloud-store

SQLite-backed persistent store for pcloud-rs: sync roots, metadata, and an
HMAC-indexed key-value table.

## What this crate does

- Wraps `rusqlite` (bundled SQLite) with a typed store API.
- Uses `hmac` + `sha2` to index sensitive lookup keys without leaking the
  plaintext of those keys into the database.
- Migrations are idempotent and run on open.

## Public API entry points

- `Store::open`, `Store::upsert_sync_root`, `Store::list_sync_roots`.
- `kv::get`, `kv::put` on the HMAC-indexed table.

## Usage

```rust,no_run
use pcloud_store::Store;

let store = Store::open_in_memory()?;
let _ = store.list_sync_roots()?;
# Ok::<(), pcloud_store::Error>(())
```

## Features

None (SQLite is always bundled).

## Security posture

- File permissions on the on-disk DB are set to `0600`.
- No raw password or auth-token columns are written by this crate.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

# pcloud-model

Shared domain-model types (files, folders, users, shares, public links) for the
pcloud-rs Rust workspace.

## What this crate does

- Defines the `serde`-friendly structs that every other crate uses to represent
  pCloud entities.
- Holds no protocol, storage, or I/O logic.

## Public API entry points

- `File`, `Folder`, `User`, `Share`, `PublicLink`, and related enums.
- Newtype IDs: `FileId`, `FolderId`, `UserId`.

## Usage

```rust
use pcloud_model::FolderId;

let root = FolderId::new(0);
assert_eq!(root.raw(), 0);
```

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

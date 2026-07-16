# pcloud-fs

Cross-platform mounted-drive integration for pcloud-rs: readdir, staging,
journal, crash-safe writeback, and native mount lifecycle adapters.

## What this crate does

- Hosts the mount-policy validation, RAII mount handles, signal-aware unmount
  cleanup, in-memory read path, staging area, and crash-safe writeback journal.
- Linux and the BSD family use `fuser`; macOS uses direct fuse-t FFI; Windows
  uses direct WinFSP FFI.
- Other Unix targets retain the portable filesystem/API layers and return an
  explicit `UnsupportedPlatform` error for kernel mounts.
- Native CI owns mount/read/write/unmount gates for each advertised mount
  backend. Passing workflow definitions are qualification evidence only after
  the corresponding jobs have actually run successfully.

## Public API entry points

- `MountHandle`, `MountPolicy`, `StagingArea`, `Journal`.

## Usage

Mount is driven by `pcloud-daemon`. Direct use outside the workspace is not
supported.

## Features

None (platform gating is handled by target cfgs).

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

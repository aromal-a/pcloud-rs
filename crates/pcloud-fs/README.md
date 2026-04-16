# pcloud-fs

Mounted-drive / FUSE integration scaffolding for pcloud-rs: readdir, staging,
journal, and writeback helpers.

## What this crate does

- Hosts the mount-policy validation, RAII mount handles, signal-aware unmount
  cleanup, in-memory read path, staging area, and crash-safe writeback journal.
- On Linux it wires to `fuser`. On other platforms the FUSE surface compiles
  out entirely.
- Remains a work-in-progress subsystem tracked under `bd-1du.4`.

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

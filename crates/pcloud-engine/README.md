# pcloud-engine

Sync-engine primitives for pcloud-rs: diff planner, scheduler scaffolding, and
state types consumed by `pcloud-daemon`.

## What this crate does

- Computes local/remote tree diffs and produces ordered work items.
- Holds the scheduler primitives that the daemon's runtime composes with
  transfer and filesystem backends.
- Is deliberately smaller than the legacy C diff engine while parity work
  continues under `bd-1du.3`.

## Public API entry points

- `DiffPlanner`, `WorkItem`, scheduler state types.

## Usage

See `crates/pcloud-daemon/src/runtime.rs` for the wiring.

## Features

None.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

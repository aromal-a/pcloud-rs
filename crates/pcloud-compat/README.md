# pcloud-compat

C-CLI compatibility shim primitives: `rpc_message_t` codec and SysV shared-memory
producer. Isolated crate, not wired into the daemon by default.

## What this crate does

- Provides byte-level parity with the legacy C client's local IPC surface so
  third-party tools pinned to the old format keep working.
- SysV shared memory is Linux-specific and reproduces the legacy 0666 world-
  accessible segment semantics. It is behind an opt-in feature.

## Public API entry points

- `rpc_message` codec functions.
- `legacy_shm` producer (feature-gated).

## Features

- `legacy-shm` — enables the SysV shm producer with legacy permissions. OFF by
  default because the legacy semantics are intentionally insecure.

## Security posture

- Default build ships no world-accessible surface.
- Enabling `legacy-shm` is a deliberate, documented compatibility choice.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

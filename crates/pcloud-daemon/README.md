# pcloud-daemon

Long-running pcloud-rs daemon (`pcloudd`): runtime orchestration, subsystem
backends, secure local IPC server, and the owner-only auth vault.

## What this crate does

- Boots the daemon from a `pcloud-config::Config`.
- Owns the runtime that drives auth, sync, transfers, shares, public links,
  backups, and crypto backends.
- Serves the `pcloud-ipc` protocol on an owner-only UNIX socket.
- Manages the persistent vault under `0600`/`0700` permissions.

## Public API entry points

- The `pcloudd` binary at `src/main.rs`.
- `bootstrap::start`, `runtime::Runtime`, and the per-subsystem `*_backend`
  modules used for integration testing.

## Usage

```text
pcloudd --config ~/.config/pcloud-rs/config.json
```

## Features

- `metrics` — enables the Prometheus exporter surface. OFF by default.
- `json-logs` — switches the log sink to structured JSON. OFF by default.

## Security posture

- Auth tokens persist only when explicitly opted in; no password persistence.
- Socket, vault file, and vault parent directory permissions are enforced and
  re-validated on every start.
- Audit and persistence failures surface as errors instead of being silently
  swallowed.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

# pcloud-backends

Subsystem backend modules (auth, sync, transfers, shares, public links,
backups, crypto, notifications, folder, mount discovery, upload journal
and state) extracted out of `pcloud-daemon` under PLAN_A_PLUS §P6.1.

## What this crate does

- Hosts the per-subsystem backends the daemon composes into its runtime.
- Keeps the daemon crate focused on bootstrap, IPC dispatch, and runtime
  orchestration while the feature logic lives here.
- Provides the canonical ID-first `RemoteFs` service plus the
  `upload_journal` and `upload_state` durability helpers.

## Public API entry points

- `auth_backend`, `sync_backend`, `transfer_backend`, `shares_backend`,
  `public_link_backend`, `backup_backend`, `crypto_backend`,
  `notifications_backend`, `folder_backend`, `account_backend`.
- `mount_discovery`, `path_resolver`, `sync_suggest`, `ignore_patterns`.
- `upload_journal`, `upload_state`.
- `RemoteFs` for resolve/stat/list/range-read/stream-write/copy/move/delete/
  mkdir/share operations.

## Usage

Consumed directly by daemon composition, sync and mount adapters, and the
internal embedded compatibility SDK. CLI, public SDK, and experimental WebDAV
reach the same service through daemon IPC. This remains a workspace-internal
API; third parties should use `pcloud-sdk`.

## Features

None.

## Security posture

- Inherits the daemon's secret-handling invariants: `SecretString` /
  `SecretBytes` throughout; no cleartext password persistence.
- Backend code must not log tokens, passwords, or keys.

## License

Dual-licensed under `MIT OR Apache-2.0`.

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

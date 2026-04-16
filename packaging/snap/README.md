<!--
PLATFORM: Linux (Snap / snapd)
STATUS: scaffolding
-->

# pcloud-rs Snap packaging

This directory contains a scaffolding `snapcraft.yaml` for publishing
`pcloud-rs` (Rust rewrite) to the Snap Store.

## Build

```bash
cd /path/to/pcloud-rs
snapcraft --use-lxd
```

Produces a `pcloud-rs_<version>_<arch>.snap` artifact for `amd64` and
`arm64` (build each architecture on its matching host or via
remote-build).

## Install locally

```bash
sudo snap install --dangerous ./pcloud-rs_*.snap
```

## Confinement trade-off

The scaffolding ships `confinement: strict`. This is **deliberately
conservative** but has a hard limitation:

- Strict-confined snaps do **not** have access to `/dev/fuse` and
  cannot create FUSE mounts on most host distributions.
- `pcloud-rs`'s mounted-drive feature (see `bd-1du.4`) therefore will
  **not** work under strict confinement.
- CLI-only use (auth, sync-root management, transfers, public links)
  is fine under strict confinement.

To enable a full FUSE-mounted experience you must:

1. Switch `confinement:` from `strict` to `classic` in
   `snapcraft.yaml`.
2. Submit the snap for **Snap Store review** — classic confinement
   requires manual approval by the Snap advisory team before the
   snap can be published to the stable channel.
3. Accept that classic-confined snaps run with host-level access,
   which is the opposite of the secure defaults the Rust rewrite
   otherwise aims for.

We keep the default `strict` so casual installs are safe, and
document the `classic` path here so the FUSE trade-off is explicit
instead of silent.

## Plugs

- `home` – user files (for sync roots and CLI config)
- `network`, `network-bind` – pCloud API + local daemon IPC
- `password-manager-service` – optional auth-vault integration

## Status

This is scaffolding. No store upload has been performed yet.

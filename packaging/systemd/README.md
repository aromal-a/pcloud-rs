# pcloudd systemd packaging

This directory ships the systemd unit files and drop-in overrides used by
the Rust pCloud daemon (`pcloudd`). The **shipped `pcloudd.service` is
intentionally strict**: it isolates `/dev`, denies all outbound network
traffic except to localhost, and filters out privileged syscall groups.
Real-world deployments almost always need at least one drop-in to widen
the policy for their use case.

## Files

| File | Purpose |
|------|---------|
| `pcloudd.service` | Main service unit. Strict sandbox. Type=notify. |
| `pcloudd.socket` | Optional socket-activation unit for `pcloudd`'s IPC. |
| `override.conf.example` | Drop-in: broaden outbound network allow list so the daemon can reach the pCloud API (`binapi.pcloud.com`, `eapi.pcloud.com`). |
| `override-fuse.conf.example` | Drop-in: relax `PrivateDevices=` and `SystemCallFilter=` so the daemon can perform FUSE mounts via `/dev/fuse`. |

## When to install each drop-in

The shipped unit enforces `IPAddressDeny=any` with only `localhost`
whitelisted, and blocks `/dev/fuse` and the `@mount` syscall family. These
defaults are correct for a conservative policy but will **prevent the
daemon from doing any real work** unless overridden.

| Deployment mode | Need `override.conf` (API access)? | Need `override-fuse.conf` (FUSE mount)? |
|-----------------|------------------------------------|-----------------------------------------|
| CLI / SDK only (no mount, no network) | No | No |
| CLI / SDK against real pCloud (API calls) | **Yes** | No |
| Mounted pCloud filesystem | **Yes** | **Yes** |

Both drop-ins can be installed side-by-side; systemd merges them
alphabetically under `pcloudd.service.d/`.

## Installation

### System unit (`DynamicUser=yes` — default):

```bash
sudo mkdir -p /etc/systemd/system/pcloudd.service.d/
sudo install -m 644 override.conf.example      \
    /etc/systemd/system/pcloudd.service.d/api-access.conf
sudo install -m 644 override-fuse.conf.example \
    /etc/systemd/system/pcloudd.service.d/fuse.conf
sudo systemctl daemon-reload
sudo systemctl enable --now pcloudd.service
```

### User unit:

```bash
mkdir -p ~/.config/systemd/user/pcloudd.service.d/
install -m 644 override.conf.example      \
    ~/.config/systemd/user/pcloudd.service.d/api-access.conf
install -m 644 override-fuse.conf.example \
    ~/.config/systemd/user/pcloudd.service.d/fuse.conf
systemctl --user daemon-reload
systemctl --user enable --now pcloudd.service
```

## Security trade-offs

- `override.conf.example` removes the `IPAddressDeny=any` + `IPAddressAllow=localhost`
  fence. TLS certificate validation and protocol-level authentication
  still apply; the trade-off is that the daemon can now open outbound
  connections to any address. Refine the `IPAddressAllow=` list to
  specific pCloud subnets if your security posture requires it.
- `override-fuse.conf.example` exposes the full `/dev` tree to the unit
  (`PrivateDevices=no`) and re-enables the `@mount` syscall group. This
  is the minimum relaxation required for FUSE mount lifecycle. If your
  kernel supports it, a tighter alternative is
  `DeviceAllow=/dev/fuse rw` + `DevicePolicy=closed` under
  `PrivateDevices=yes` — but that requires system-mode with
  `CAP_SYS_ADMIN` and a cgroup-capable init.

Do **not** install either drop-in "just in case" — only install what the
deployment actually needs. The shipped defaults are the safe baseline.

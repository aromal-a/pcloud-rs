# pcloudd systemd packaging

This directory ships the systemd unit files and drop-in overrides used by
the Rust pCloud daemon (`pcloudd`). The **shipped `pcloudd.service` is
sandbox-strict**: it isolates `/dev`, runs under `DynamicUser=`, applies
`ProtectSystem=strict`, and filters out privileged syscall groups.
Outbound network traffic is gated by the host firewall, not by the unit
(this is a deliberate change as of 2026-04-30 — see CLAUDEREV iter-2
DEPLOY-H-11.3 fix). FUSE-mounted-drive deployments still need the
`override-fuse.conf.example` drop-in; egress allow-listing via
`override.conf.example` is OPT-IN defence-in-depth. Per-user deployments
use `pcloudd-user.service`, not the system unit.

## Files

| File | Purpose |
|------|---------|
| `pcloudd.service` | System service unit. Strict sandbox, `DynamicUser=yes`, `Type=simple`. |
| `pcloudd-user.service` | Per-user service unit for `systemctl --user`; avoids system-only directives. |
| `pcloudd.socket` | Optional socket-activation unit for `pcloudd`'s IPC. |
| `override.conf.example` | OPT-IN drop-in: add a strict cgroup-level egress allow-list on top of the host firewall (`IPAddressDeny=any` + targeted `IPAddressAllow=` for `binapi.pcloud.com` / `eapi.pcloud.com`). The shipped unit no longer sets `IPAddressDeny` by default. |
| `override-fuse.conf.example` | Drop-in: relax `PrivateDevices=` and `SystemCallFilter=` so the daemon can perform FUSE mounts via `/dev/fuse`. |
| `override-user.conf.example` | Legacy compatibility drop-in if an operator has already copied `pcloudd.service` into a user unit. Prefer `pcloudd-user.service` for new installs. |

## When to install each drop-in

The shipped unit blocks `/dev/fuse` and the `@mount` syscall family by
default. As of 2026-04-30 the unit no longer sets `IPAddressDeny=any` —
outbound network is gated by the host firewall, not the unit (CLAUDEREV
iter-2 DEPLOY-H-11.3 fix; iter-3 doc reconciliation).

| Deployment mode | Need `override.conf` (egress allow-list)? | Need `override-fuse.conf` (FUSE mount)? |
|-----------------|-------------------------------------------|-----------------------------------------|
| CLI / SDK only (no mount, no network) | No | No |
| CLI / SDK against real pCloud (API calls) | No (host firewall is the gate; install only for cgroup-level defence-in-depth) | No |
| Mounted pCloud filesystem | No (same as above) | **Yes** |

Both drop-ins can be installed side-by-side; systemd merges them
alphabetically under `pcloudd.service.d/`.

## Installation

### System unit (`DynamicUser=yes` — default):

```bash
sudo install -Dm0644 pcloudd.service /etc/systemd/system/pcloudd.service
sudo systemctl daemon-reload
sudo systemctl enable --now pcloudd.service

# Optional strict egress allow-list:
sudo install -Dm0644 override.conf.example \
    /etc/systemd/system/pcloudd.service.d/egress-allow-list.conf

# Optional FUSE mount support:
sudo install -Dm0644 override-fuse.conf.example \
    /etc/systemd/system/pcloudd.service.d/fuse.conf
sudo systemctl daemon-reload
sudo systemctl restart pcloudd.service
```

### User unit:

```bash
install -Dm0644 pcloudd-user.service ~/.config/systemd/user/pcloudd.service
systemctl --user daemon-reload
systemctl --user enable --now pcloudd.service

# Optional strict egress allow-list:
install -Dm0644 override.conf.example \
    ~/.config/systemd/user/pcloudd.service.d/egress-allow-list.conf

# Optional FUSE mount support:
install -Dm0644 override-fuse.conf.example \
    ~/.config/systemd/user/pcloudd.service.d/fuse.conf
systemctl --user daemon-reload
systemctl --user restart pcloudd.service
```

## Security trade-offs

- `override.conf.example` is OPT-IN: it adds an `IPAddressDeny=any` +
  targeted `IPAddressAllow=` list as a cgroup-level egress filter on top
  of the host firewall. The shipped unit no longer carries that fence by
  default (changed 2026-04-30 to fix the silent-block on a default
  install). When installed, the trade-off is the operational burden of
  keeping `IPAddressAllow=` in sync with pCloud's published API CIDRs;
  TLS certificate validation and protocol-level authentication apply
  regardless.
- `override-fuse.conf.example` exposes the full `/dev` tree to the unit
  (`PrivateDevices=no`) and re-enables the `@mount` syscall group. This
  is the minimum relaxation required for FUSE mount lifecycle. If your
  kernel supports it, a tighter alternative is
  `DeviceAllow=/dev/fuse rw` + `DevicePolicy=closed` under
  `PrivateDevices=yes` — but that requires system-mode with
  `CAP_SYS_ADMIN` and a cgroup-capable init.

Do **not** install either drop-in "just in case" — only install what the
deployment actually needs. The shipped defaults are the safe baseline.

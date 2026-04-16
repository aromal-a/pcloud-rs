# Linux

Platform notes for running `pcloud-daemon` and `pcloud-cli` on Linux.
Linux is the reference platform for the Rust rewrite and has the most
complete parity coverage.

## Support status

- **Tier 1, live-tested.** Linux is the only platform where the mount
  path is end-to-end verified on real hardware. See the authoritative
  support matrix in
  [`architecture/platform-support.md`](../../architecture/platform-support.md).
- Status legend used on this page: **Live-tested** means a human has
  booted, mounted, authenticated, and exercised the CLI against a live
  pCloud account on the stated OS version.

> **Landing status (2026-04-15):** Tier 1. P0–P5 of the cross-platform
> plan (see [`PLAN_CROSSPLATFORM.md`](../../../../../PLAN_CROSSPLATFORM.md))
> are landed and live-verified on Linux: `fuser` mount adapter, all FUSE
> callbacks (read + write path), systemd unit, and nfpm/AUR/Nix/Flatpak/
> Snap/Docker/AppImage packaging. See
> [Packaging reference](../../reference/packaging.md) for the full channel
> matrix.

## OS version matrix

| Distribution          | Version                 | Kernel      | Status         |
|-----------------------|-------------------------|-------------|----------------|
| Ubuntu                | 22.04 LTS, 24.04 LTS    | 5.15 / 6.8  | Live-tested    |
| Debian                | 12 (bookworm)           | 6.1         | Live-tested    |
| Fedora                | 39, 40                  | 6.6 / 6.8   | Live-tested    |
| RHEL / Rocky / Alma   | 9.x                     | 5.14        | Live-tested    |
| Arch Linux            | rolling                 | >= 6.6      | Live-tested    |
| openSUSE Leap         | 15.5                    | 5.14        | Live-tested    |
| Alpine                | 3.19 (glibc only)       | 6.6         | Scaffolded, musl build not gated |
| RHEL 7 / CentOS 7     | 3.10                    | 3.10        | **Not supported** — FUSE3 missing |
| Kernel < 5.4          | any                     | <5.4        | **Not supported** — see known gaps |

Anything not listed is best-effort. File a bead with the exact
`uname -a`, distro, and systemd version.

## Install

### Package managers

Pick the channel matching your distribution:

```bash
# Debian / Ubuntu (project APT repo)
curl -fsSL https://apt.pcloud-rs.example/pubkey.asc | \
  sudo tee /etc/apt/keyrings/pcloud-rs.asc >/dev/null
echo "deb [signed-by=/etc/apt/keyrings/pcloud-rs.asc] \
  https://apt.pcloud-rs.example stable main" | \
  sudo tee /etc/apt/sources.list.d/pcloud-rs.list
sudo apt update && sudo apt install pcloud-rs

# Fedora / RHEL / Rocky / Alma
sudo dnf install pcloud-rs

# openSUSE Leap / Tumbleweed
sudo zypper install pcloud-rs

# Arch (AUR)
yay -S pcloud-rs
# or pacman once in community
sudo pacman -S pcloud-rs

# Nix / NixOS
nix profile install github:pcloud-rs/pcloud-rs#pcloud-rs

# Alpine (edge testing)
doas apk add pcloud-rs
```

### From source

Build times on a modern 8-core laptop (Zen 3, 32 GiB RAM, NVMe):

- Clean release build: **4–6 minutes.**
- Incremental recompile after touching one crate: **10–40 seconds.**

```bash
# Install toolchain and FUSE3 development headers
sudo apt install build-essential pkg-config libfuse3-dev libssl-dev \
  ca-certificates curl git
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"

# Build
git clone https://github.com/pcloud-rs/pcloud-rs
cd pcloud-rs/
cargo build --release -p pcloud-daemon -p pcloud-cli

install -Dm0755 target/release/pcloud-daemon \
  ~/.local/bin/pcloud-daemon
install -Dm0755 target/release/pcloud-cli \
  ~/.local/bin/pcloudc
```

### Verification

Every release artifact must be signature- and hash-verified before
execution:

```bash
sha256sum -c SHA256SUMS.txt
cosign verify-blob --key release.pub \
  --signature pcloud-daemon.sig pcloud-daemon
```

## Config paths (XDG)

The Linux build is strict XDG. If `XDG_*` is unset the daemon falls
back to the defaults listed below; it does **not** quietly use `/tmp`
or `$HOME` for secret material.

| Role               | Path                                                     | Mode  |
|--------------------|----------------------------------------------------------|-------|
| Config             | `$XDG_CONFIG_HOME/pcloud-rs/config.toml` (`~/.config/pcloud-rs/config.toml`) | 0600  |
| State (store)      | `$XDG_DATA_HOME/pcloud-rs/store.sqlite` (+`-wal`,`-shm`) (`~/.local/share/pcloud-rs/`) | 0600  |
| Vault              | `$XDG_DATA_HOME/pcloud-rs/vault.dat`                       | 0600  |
| Journal            | `$XDG_DATA_HOME/pcloud-rs/journal/`                        | 0700  |
| Cache (disposable) | `$XDG_CACHE_HOME/pcloud-rs/`                               | 0700  |
| IPC socket         | `$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock`                   | 0600  |
| Log (if file)      | `$XDG_STATE_HOME/pcloud-rs/daemon.log`                     | 0600  |

Parent directories for secret-bearing files are `0700` and owned by
the running UID. The daemon refuses to start against a vault whose
ownership or mode disagrees — do **not** `chmod` to paper over this.

Create the state directories once with correct permissions:

```bash
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs
```

## Service management (systemd)

The distribution packages ship a **user** unit by default:
`pcloud-rs-daemon.service` at `~/.config/systemd/user/` or
`/usr/lib/systemd/user/`. Running as a user service is the
recommended mode because the daemon handles per-user secrets.

```bash
# Enable and start on login
systemctl --user enable --now pcloud-rs-daemon

# Inspect
systemctl --user status pcloud-rs-daemon
journalctl --user -u pcloud-rs-daemon -f

# Stop and disable
systemctl --user stop pcloud-rs-daemon
systemctl --user disable pcloud-rs-daemon
```

For multi-user workstations, enable lingering so the daemon survives
logout:

```bash
sudo loginctl enable-linger "$USER"
```

The unit sets sane hardening:

```ini
[Service]
Type=notify
ExecStart=/usr/bin/pcloud-daemon --log-format json --log-level info
Restart=on-failure
RestartSec=5s
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/.config/pcloud-rs %h/.local/share/pcloud-rs %h/.cache/pcloud-rs
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
MemoryDenyWriteExecute=yes
LockPersonality=yes
SystemCallArchitectures=native
```

Do not relax these without a bead justifying it — they are part of the
secure-by-default posture.

A system-scoped unit (`pcloud-rs-daemon@<user>.service`) exists for
managed fleets; it still runs the daemon as the target user, never
as root.

## Mount setup (FUSE3)

Linux mounts require a working FUSE3 userspace:

```bash
# Debian/Ubuntu
sudo apt install fuse3
# Fedora
sudo dnf install fuse3
# Arch
sudo pacman -S fuse3
```

Verify:

```bash
fusermount3 --version
```

Ensure the invoking user is allowed to mount FUSE (most distros grant
this by default). If your distro still uses `/etc/fuse.conf`,
`user_allow_other` is **not** required for per-user mounts in the
default Rust policy.

Enable the mount in `config.toml`:

```toml
[mount]
enabled = true
path    = "/home/alice/pCloudDrive"
policy  = "default"

# Cache tuning (optional — all have sane defaults)
cache_size_mb       = 256   # page-cache memory budget in MiB
page_cache_entries  = 4096  # max metadata-cache entries (LRU)
metadata_ttl_secs   = 60    # metadata-cache TTL; 0 disables caching
```

Or via CLI:

```bash
pcloudc mount --path ~/pCloudDrive
pcloudc mount --status
pcloudc mount --unmount
```

Wedged mount recovery — see
[runbook.md Playbook 7](../runbook.md#playbook-7-kernel-mount-recovery):

```bash
fusermount3 -u -z ~/pCloudDrive
PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloud-rs-daemon
```

### FUSE status (2026-04-16)

- **Live read + write** through both mount paths:
  - **`FuserShim<A>` / `BoxedFuserShim`** (dyn-trait path in
    `pcloud-fs::platform::linux`): all read-path ops (`lookup` /
    `getattr` / `readdir` / `open` / `read` / `release`) **and**
    write-path ops (`create` / `write` / `flush` / `fsync` /
    `setattr(size)` / `unlink` / `rename` / `mkdir` / `rmdir`) are
    forwarded through the `FuseAdapter` trait. When the adapter has a
    `WritePathService` attached, writes work; otherwise the trait
    defaults return `ENOSYS` and the mount is effectively read-only.
    Exercised by `crates/pcloud-fs/tests/fuse_dyn_shim_write.rs`
    (gated on `PCLOUD_FUSE_TEST=1`).
  - **`PcloudFsShim`** (concrete composition path used by the daemon):
    `create` / `write` / `flush` / `fsync` / `unlink` / `rename` /
    `setattr(size)` + `mkdir` / `rmdir` are forwarded to a crash-safe
    write journal and upload pipeline. Exercised by
    `crates/pcloud-fs/tests/fuse_kernel_e2e.rs` (64 MiB kernel
    round-trip, same gate).
- **Performance follow-up:** chunked `upload_write` pipelining for
  sustained multi-GiB writes (`TODO(bd-1du.4.6)` in `write_path.rs`).

Mounted-drive parity remains tracked under `bd-1du.4`; the read path
has landed and is live-verified, the write path (via
`PcloudFsShim`) has landed at the code level but the final parity
proof / release gate remains `bd-1du.10`.

## Vault backend

The Linux vault backend is a file-backed vault at
`~/.local/share/pcloud-rs/vault.dat`, mode `0600`, parent dir `0700`,
UID-bound, ownership and mode validated on every open. There is **no**
Secret Service / GNOME Keyring / kwallet integration on Linux at this
time — adding one is tracked as a future bead. Secret material is
held in `SecretString` / `SecretBytes` wrappers in memory and zeroized
on drop.

If you need a hardware-backed root, provision the user on an encrypted
home (LUKS, fscrypt, eCryptfs) — the vault inherits the protection of
the underlying filesystem.

## Upgrade

See [Upgrade](../upgrade.md) for the semver policy and the 2-wave
rolling upgrade. Quick path:

```bash
pcloudc --json status > /tmp/pre.json
systemctl --user stop pcloud-rs-daemon
sudo apt install --only-upgrade pcloud-rs
systemctl --user start pcloud-rs-daemon
pcloudc doctor --json
pcloudc status              # auth=Authenticated, healthy engine summary
```

## Uninstall

```bash
# 1. Stop and disable the service
systemctl --user stop pcloud-rs-daemon
systemctl --user disable pcloud-rs-daemon

# 2. Remove the package
sudo apt remove pcloud-rs         # Debian/Ubuntu
sudo dnf remove pcloud-rs         # Fedora
sudo pacman -Rns pcloud-rs        # Arch

# 3. Remove per-user state (this deletes the vault)
rm -rf \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.cache/pcloud-rs

# 4. Remove the runtime socket dir if present
rm -rf "$XDG_RUNTIME_DIR/pcloud-rs"
```

A clean uninstall leaves no `pcloud-daemon` processes, no FUSE mounts,
no systemd units, no state directories. Verify:

```bash
pgrep -a pcloud-daemon || echo "clean"
mount | grep -i pcloud || echo "clean"
```

## First-run bootstrap

Beginner path (one-user workstation, no fleet, no systemd hardening
tuning):

```bash
# 1. Ensure the invoking user can mount FUSE. On most distros the
#    user's primary group already has /dev/fuse rw+.
id -nG | tr ' ' '\n' | grep -E '^(fuse|kvm|plugdev)$' || true

# If /dev/fuse is 0600 root:fuse, add the user to the fuse group:
sudo groupadd -f fuse
sudo gpasswd -a "$USER" fuse
# log out and back in so the new group membership takes effect

# 2. Create state dirs with the correct permissions (idempotent):
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs

# 3. Enable the user service
systemctl --user enable --now pcloud-rs-daemon

# 4. Sanity check
pcloudc doctor --json | jq '.checks[] | {name, status}'
pcloudc status
```

FAANG-ops tuning callouts:

- Prefer a dedicated service user (`pcloud-rs`) on shared build boxes
  and wrap the user unit in `systemd-nspawn` or a rootless Podman
  container that still mounts `/dev/fuse` and `/run/user/$UID`.
- Capture `NeedsReload=yes` reconfiguration via `Drop-Ins/` rather
  than editing the shipped unit.
- Set `CPUAccounting=yes MemoryAccounting=yes IOAccounting=yes` at
  the unit level for cgroup-v2 telemetry; the daemon emits its own
  counters but having cgroup accounting makes fleet-wide regressions
  much easier to pin.

## Service management cheat-sheet

| Action              | Command                                                     |
|---------------------|-------------------------------------------------------------|
| Start               | `systemctl --user start pcloud-rs-daemon`                    |
| Stop                | `systemctl --user stop pcloud-rs-daemon`                     |
| Enable at login     | `systemctl --user enable pcloud-rs-daemon`                   |
| Disable             | `systemctl --user disable pcloud-rs-daemon`                  |
| Restart             | `systemctl --user restart pcloud-rs-daemon`                  |
| Status              | `systemctl --user status pcloud-rs-daemon`                   |
| Follow logs         | `journalctl --user -u pcloud-rs-daemon -f`                   |
| Last 24h errors     | `journalctl --user -u pcloud-rs-daemon --since '24h ago' -p err` |
| Core-dump inspect   | `coredumpctl list pcloud-daemon` then `coredumpctl gdb <PID>` |

Enable core dumps (once, system-wide):

```bash
sudo mkdir -p /var/lib/systemd/coredump
sudo sysctl -w kernel.core_pattern='|/lib/systemd/systemd-coredump %P %u %g %s %t %c %h'
```

Core-dump capture for the user service:

```bash
systemctl --user edit pcloud-rs-daemon
# add:
# [Service]
# LimitCORE=infinity
systemctl --user daemon-reload
```

## Peer-cred and IPC

- Transport: `AF_UNIX` stream socket at
  `$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock`, mode `0600`, parent dir
  `0700`, both UID-checked on every connection.
- Peer identity: the daemon calls `getsockopt(SO_PEERCRED)` on Linux
  and **rejects** any peer whose UID does not match the daemon's own
  UID. There is no password fallback on the local channel; trust is
  derived entirely from the kernel-reported `ucred`.
- CLI discovery: `pcloudc` honours `$PCLOUD_SOCKET` first, then falls
  back to `$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock`.
- Relevant crates: `pcloud-ipc/src/transport.rs`,
  `pcloud-ipc/src/server.rs` (see `tests/peer_and_protocol.rs` for
  the enforced contract).

Do not relax socket permissions. Fleet operators who need to let a
monitoring agent call the daemon should make that agent run as the
same UID (preferred) or run a separate `pcloudc`-based sidecar.

## Secret storage backend

- In-memory: `SecretString` / `SecretBytes` from `pcloud-secret`
  (zeroize-on-drop, redacted `Debug`).
- On-disk: file-backed vault at
  `~/.local/share/pcloud-rs/vault.dat`, mode `0600`, parent `0700`,
  ownership + mode re-validated on every open.
- **Secret Service (GNOME Keyring / KWallet) is _not_ wired.** This is
  intentional at the moment — tracked as a future bead. Adding it
  must not weaken the file-vault posture.
- Hardware root of trust: LUKS / fscrypt / eCryptfs on the user's
  home directory. The vault inherits those protections.

Never persist plaintext passwords. The legacy C client's
`pcloud-rs_save_password` behavior is intentionally _not_ mirrored.

## Observability integration

- Default log sink: **stderr in JSON** when the daemon is a child of
  systemd, which journald captures natively.
- `journalctl --user -u pcloud-rs-daemon --output=json | \
    jq 'select(.PRIORITY<="3")'` surfaces warnings and errors.
- Structured log keys the fleet tooling can rely on:
  `event`, `component`, `corr_id`, `auth_state`, `mount_state`,
  `engine_state`, `bytes_in`, `bytes_out`, `duration_ms`.
- Metrics endpoint: off by default. Flip on with
  `[telemetry] metrics_addr = "127.0.0.1:9131"` in `config.toml`.
  The Prometheus text-format scrape emits counters and histograms for
  auth, FS, transfers, and engine.
- Audit trail: when durable audit is enabled, the audit log lives
  under `$XDG_STATE_HOME/pcloud-rs/audit.log` with the same mode
  enforcement as the vault.

## Firewall, SELinux, AppArmor, sandboxing

### Outbound network

The daemon makes **outbound-only** HTTPS connections (443) to
`*.pcloud.com` API endpoints. No inbound network surface. Egress
firewalls should allow:

```
TCP/443 → api.pcloud.com
TCP/443 → eapi.pcloud.com
TCP/443 → binapi.pcloud.com
TCP/443 → <regional upload/download hosts returned by getfilelink>
```

`getfilelink` returns dynamic CDN hosts; overly-strict egress ACLs
that pin a single hostname will cause transfer failures. Observe the
set with one authenticated session and allowlist on apex domains.

### SELinux (Fedora / RHEL / Rocky)

Default `targeted` policy works out of the box. If you confine the
daemon further:

```bash
# Permissive for our domain only (test)
sudo semanage permissive -a pcloud_daemon_t
# Allow FUSE mounts under $HOME
sudo setsebool -P use_fusefs_home_dirs on
```

If audit2allow flags violations under `~/.local/share/pcloud-rs`,
prefer a user-type transition over relaxing the global policy.

### AppArmor (Debian / Ubuntu)

No profile ships by default. If you author one, grant:

```
owner @{HOME}/.config/pcloud-rs/** rw,
owner @{HOME}/.local/share/pcloud-rs/** rw,
owner @{HOME}/.cache/pcloud-rs/** rw,
/dev/fuse rw,
@{PROC}/@{pid}/mountinfo r,
@{run}/user/@{uid}/pcloud-rs/** rw,
network inet stream,
```

### nftables / iptables

Host firewalls are not load-bearing because the daemon does not
listen on any IP socket in the default configuration. If you enable
the optional Prometheus metrics port, restrict it to `127.0.0.1`.

## Troubleshooting (top 10)

1. **`Connection refused` on `pcloudc`** — daemon not running.
   ```bash
   systemctl --user status pcloud-rs-daemon
   journalctl --user -u pcloud-rs-daemon -n 200 --no-pager
   ```
2. **`EACCES` on socket** — wrong UID or relaxed perms.
   ```bash
   ls -la "$XDG_RUNTIME_DIR/pcloud-rs/"
   stat -c '%U:%G %a' "$XDG_RUNTIME_DIR/pcloud-rs/daemon.sock"
   ```
   Expected `0600`, owned by the invoking UID. Fix by restarting.
3. **`Vault rejected (mode 0644)`** — someone chmod'd the vault.
   ```bash
   chmod 0600 ~/.local/share/pcloud-rs/vault.dat
   chmod 0700 ~/.local/share/pcloud-rs
   ```
4. **Mount hangs, `ls ~/pCloudDrive` blocks** — stale FUSE.
   ```bash
   fusermount3 -u -z ~/pCloudDrive
   PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloud-rs-daemon
   ```
5. **`SQLITE_BUSY`** — state on NFS/CIFS. Move `XDG_DATA_HOME` to a
   local disk and restart.
6. **TLS errors to `api.pcloud.com`** — missing CA bundle or MITM
   proxy. Verify:
   ```bash
   curl -fsSI https://api.pcloud.com/getip
   ```
7. **`auth=NeedsTFA` on every start** — vault missing or not
   persisted.
   ```bash
   pcloudc auth status
   pcloudc auth login --persist --keyfile ~/.config/pcloud-rs/auth.key
   ```
8. **Daemon OOM-killed** — cap the unit.
   ```bash
   systemctl --user edit pcloud-rs-daemon
   # [Service]
   # MemoryHigh=1.5G
   # MemoryMax=2G
   ```
9. **FUSE refuses to mount, `permission denied`** — user not in the
   `fuse` group or `/etc/fuse.conf` tightened. Check
   `ls -la /dev/fuse` (expect `crw-rw-rw-` or group-writable).
10. **Clock skew → 401** — the API rejects signatures with
    large clock drift. Install `chrony` or `systemd-timesyncd`:
    ```bash
    sudo systemctl enable --now systemd-timesyncd
    timedatectl status
    ```

## Upgrading

- In-place package upgrades within a minor series are safe; the
  daemon applies SQLite migrations on first start. Always snapshot
  `~/.local/share/pcloud-rs/store.sqlite` before a major-version bump.
- Two-wave rolling strategy for managed fleets is documented in
  [Upgrade](../upgrade.md); the platform-specific hook here is that
  `systemctl --user daemon-reload` must run after swapping the unit
  file in `/usr/lib/systemd/user/`.

## Uninstalling

See the **Uninstall** section below for the step-by-step removal.

## Known gaps (Linux)

- No Secret Service / Keyring integration yet.
- No Wayland-native credential prompt.
- No native containerised (rootless) mount profile — runs under
  Docker/Podman only if `/dev/fuse` and `/run/user/$UID` are mounted.
- FUSE3 is required; FUSE2-only distros (very old LTS) are not
  supported and no backport is planned.

## Known issues

- **SELinux labeling.** On SELinux-enforcing distros, the daemon runs
  under `user_u:user_r:user_t`; the default policy allows everything
  the daemon needs. If you confine the daemon further, ensure it can
  create FUSE mounts and bind to `$XDG_RUNTIME_DIR`.
- **AppArmor.** Debian/Ubuntu AppArmor profiles may block
  `/proc/self/mountinfo` reads in unusual setups. The daemon uses
  this to reject nested sync roots; if it is blocked, sync-root
  registration works but loses the nested-root safety check — keep
  the profile permissive for `@{PROC}/@{pid}/mountinfo`.
- **Snap / Flatpak packaging.** Not supported. The daemon needs
  access to user directories and FUSE that sandboxes block by
  default. Use the native distro packages.
- **Home on a network mount.** If `$HOME` is on NFS/CIFS, the store
  may hit `SQLITE_BUSY` under contention. Either move state to a
  local path via `XDG_DATA_HOME` or accept slower write throughput.
- **Kernel < 5.4.** FUSE3 behavior degrades noticeably. File a bead
  if you must support older kernels in your fleet.
- **Wayland clipboard paste of vault path.** Pasting paths containing
  user names into commands that get logged can leak minor PII; use
  `$HOME` relative paths in runbook commands.

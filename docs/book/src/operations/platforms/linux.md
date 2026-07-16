# Linux

Platform notes for running `pcloudd` and `pcloudc` on Linux.
Linux is the reference platform for the Rust rewrite and has the most
complete parity coverage.

## Support status

- **Tier 1 target, locally kernel-tested.** Linux is the only platform where
  the current worktree's mount path has been exercised against a real kernel
  device. That run used an in-process deterministic backend, not live pCloud
  credentials or an installed release package. See the authoritative
  support matrix in
  [`architecture/platform-support.md`](../../architecture/platform-support.md).
- Status legend used on this page: **Local kernel-tested** means the ignored
  real-`/dev/fuse` suite passed on the stated host. **Release-qualified** would
  additionally require a clean release-commit job, installed-package lifecycle,
  and credentialed pCloud smoke test; no row currently has that status.

> **Landing status (2026-07-16):** Linux is the reference platform, but the
> published channel set is empty. Tag workflows are defined to build raw Linux
> x86_64 `pcloudd` / `pcloudc` binaries plus `.deb` and `.rpm` packages, but no
> public release exists. AppImage, Flatpak, Snap, Docker, AUR, and distro
> repositories are scaffolds unless the packaging matrix says otherwise.

## OS version matrix

| Distribution          | Version                 | Kernel      | Status         |
|-----------------------|-------------------------|-------------|----------------|
| Ubuntu                | 22.04 LTS, 24.04 LTS    | 5.15 / 6.8  | Target; no current release-commit evidence |
| Debian                | 12 (bookworm)           | 6.1         | Target; package smoke pending |
| Fedora                | 39, 40                  | 6.6 / 6.8   | Target; package smoke pending |
| RHEL / Rocky / Alma   | 9.x                     | 5.14        | Target; no current release-commit evidence |
| Arch Linux            | rolling                 | 6.19.11     | Local kernel-tested on 2026-07-16 |
| openSUSE Leap         | 15.5                    | 5.14        | Target; no current release-commit evidence |
| Alpine                | 3.19 (glibc only)       | 6.6         | Scaffolded, musl build not gated |
| RHEL 7 / CentOS 7     | 3.10                    | 3.10        | **Not supported** — FUSE3 missing |
| Kernel < 5.4          | any                     | <5.4        | **Not supported** — see known gaps |

Anything not listed is best-effort. File a bead with the exact
`uname -a`, distro, and systemd version.

## Install

### Current availability

There are no current release artifacts. Nix users can build the checked-out
source with `nix build .#pcloud-rs`; everyone else should use the source-build
instructions below. No project APT/YUM/zypper repository, AUR package, Alpine
package, GHCR image, Flatpak, Snap, AppImage, `.deb`, or `.rpm` is published
today. See
[Packaging matrix](../packaging-matrix.md) for the authoritative channel
status.

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
git clone https://github.com/ezechiel203/pcloud-rs
cd pcloud-rs/
cargo build --release --locked -p pcloud-daemon -p pcloud-cli

install -Dm0755 target/release/pcloudd \
  ~/.local/bin/pcloudd
install -Dm0755 target/release/pcloudc \
  ~/.local/bin/pcloudc
```

### Verification

Every release artifact must be signature- and hash-verified before
execution:

```bash
sha256sum -c SHA256SUMS.txt
cosign verify-blob --key release.pub \
  --signature pcloudd.sig pcloudd
```

## Config paths (XDG)

The Linux build is strict XDG. If `XDG_*` is unset the daemon falls
back to the defaults listed below; it does **not** quietly use `/tmp`
or `$HOME` for secret material.

| Role               | Path                                                     | Mode  |
|--------------------|----------------------------------------------------------|-------|
| Config file candidate | `$HOME/.config/pcloud/config.json`, then `$HOME/.pcloud/config.json` | 0600 |
| Config directory | `$XDG_CONFIG_HOME/pcloud/pcloud-rs` | 0700 |
| State directory | `$XDG_DATA_HOME/pcloud/pcloud-rs` | 0700 |
| Cache directory | `$XDG_CACHE_HOME/pcloud/pcloud-rs` | 0700 |
| Runtime directory | `$XDG_RUNTIME_DIR/pcloud/pcloud-rs` (fallback: `<cache>/pcloud-rs-runtime`) | 0700 |
| IPC socket | `<runtime_dir>/pcloud.sock` | 0600 |

Parent directories for secret-bearing files are `0700` and owned by
the running UID. The daemon refuses to start against a vault whose
ownership or mode disagrees — do **not** `chmod` to paper over this.

Create the state directories once with correct permissions:

```bash
install -d -m 0700 \
  ~/.config/pcloud/pcloud-rs \
  ~/.local/share/pcloud/pcloud-rs \
  ~/.cache/pcloud/pcloud-rs
```

## Service management (systemd)

The `.deb` / `.rpm` packages install a **system** unit:
`/lib/systemd/system/pcloudd.service`. It uses `DynamicUser=yes` and is
appropriate for headless/service-account deployments. For an interactive
per-user daemon, install `packaging/systemd/pcloudd-user.service` as
`~/.config/systemd/user/pcloudd.service`.

```bash
# Per-user install from the source tree
install -Dm0644 packaging/systemd/pcloudd-user.service \
  ~/.config/systemd/user/pcloudd.service
systemctl --user daemon-reload
systemctl --user enable --now pcloudd.service

# Inspect
systemctl --user status pcloudd.service
journalctl --user -u pcloudd.service -f

# Stop and disable
systemctl --user stop pcloudd.service
systemctl --user disable pcloudd.service
```

For multi-user workstations, enable lingering so the daemon survives
logout:

```bash
sudo loginctl enable-linger "$USER"
```

The system unit sets strict hardening:

```ini
[Service]
Type=simple
ExecStart=/usr/bin/pcloudd serve
Restart=on-failure
RestartSec=5s
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=tmpfs
DynamicUser=yes
StateDirectory=pcloud-rs
RuntimeDirectory=pcloud-rs
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

No templated `pcloudd@<user>.service` ships today.

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

Enable mount policy in the JSON config envelope (`config.json`) if you need
non-default access policy or cache sizing. Excerpt:

```json
{
  "profile": {
    "mount": {
      "allow_other": false,
      "owner_only_by_default": true,
      "cache_size_mb": 256,
      "page_cache_entries": 4096,
      "metadata_ttl_secs": 60
    }
  }
}
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
PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloudd.service
```

### FUSE status (2026-07-16)

The current practical Linux aggregate is:

```bash
PCLOUD_FUSE_TEST=1 PCLOUD_STRICT_FUSE_TEST=1 \
  cargo test -p pcloud-fs --locked -- --ignored \
    --skip chunked_flush_sustains_2gib_write_with_transient_retry \
    --nocapture --test-threads=1
```

On Arch Linux x86_64, kernel 6.19.11 and FUSE 3.18.2, all 16 selected
mount/probe tests passed and `findmnt` reported no remaining FUSE mount. The
suite includes the 64 MiB create/write/fsync/read/rename/unlink test,
write-unmount-remount byte identity, concurrent mounts, forced-detach cleanup,
metadata-size publication, and clean journal checkpointing. The skipped 2 GiB
case is a resource-intensive upload stress test, not a kernel-mount lifecycle
test. It was run separately on the same date and passed in 22.23 seconds,
including the injected transient failure, exact offset replay, and full 2 GiB
byte accounting. Release candidates must repeat both commands.

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
- **Remaining qualification:** run the aggregate on the clean release commit,
  repeat the separate 2 GiB stress gate, install and remove the produced
  packages, and execute credentialed remote transfer/share/mount smoke tests.

## Vault backend

The Linux vault backend is a file-backed vault at
the auth-token vault under the managed config directory, mode `0600`,
parent dir `0700`,
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
systemctl --user stop pcloudd.service
sudo apt install --only-upgrade pcloud-rs
systemctl --user start pcloudd.service
pcloudc doctor --json
pcloudc status              # auth=Authenticated, healthy engine summary
```

## Uninstall

```bash
# 1. Stop and disable the service
systemctl --user stop pcloudd.service
systemctl --user disable pcloudd.service

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

A clean uninstall leaves no `pcloudd` processes, no FUSE mounts,
no systemd units, no state directories. Verify:

```bash
pgrep -a pcloudd || echo "clean"
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
  ~/.config/pcloud/pcloud-rs \
  ~/.local/share/pcloud/pcloud-rs \
  ~/.cache/pcloud/pcloud-rs

# 3. Enable the user service
install -Dm0644 packaging/systemd/pcloudd-user.service \
  ~/.config/systemd/user/pcloudd.service
systemctl --user daemon-reload
systemctl --user enable --now pcloudd.service

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
| Start               | `systemctl --user start pcloudd.service`                    |
| Stop                | `systemctl --user stop pcloudd.service`                     |
| Enable at login     | `systemctl --user enable pcloudd.service`                   |
| Disable             | `systemctl --user disable pcloudd.service`                  |
| Restart             | `systemctl --user restart pcloudd.service`                  |
| Status              | `systemctl --user status pcloudd.service`                   |
| Follow logs         | `journalctl --user -u pcloudd.service -f`                   |
| Last 24h errors     | `journalctl --user -u pcloudd.service --since '24h ago' -p err` |
| Core-dump inspect   | `coredumpctl list pcloudd` then `coredumpctl gdb <PID>` |

Enable core dumps (once, system-wide):

```bash
sudo mkdir -p /var/lib/systemd/coredump
sudo sysctl -w kernel.core_pattern='|/lib/systemd/systemd-coredump %P %u %g %s %t %c %h'
```

Core-dump capture for the user service:

```bash
systemctl --user edit pcloudd.service
# add:
# [Service]
# LimitCORE=infinity
systemctl --user daemon-reload
```

## Peer-cred and IPC

- Transport: `AF_UNIX` stream socket at
  `<runtime_dir>/pcloud.sock`, mode `0600`, parent dir
  `0700`, both UID-checked on every connection.
- Peer identity: the daemon calls `getsockopt(SO_PEERCRED)` on Linux
  and **rejects** any peer whose UID does not match the daemon's own
  UID. There is no password fallback on the local channel; trust is
  derived entirely from the kernel-reported `ucred`.
- CLI discovery: `pcloudc` uses the managed runtime directory derived from
  the same XDG / `PCLOUD_ROOT` rules as the daemon.
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
  the managed auth-token vault, mode `0600`, parent `0700`,
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
- `journalctl --user -u pcloudd.service --output=json | \
    jq 'select(.PRIORITY<="3")'` surfaces warnings and errors.
- Structured log keys the fleet tooling can rely on:
  `event`, `component`, `corr_id`, `auth_state`, `mount_state`,
  `engine_state`, `bytes_in`, `bytes_out`, `duration_ms`.
- Metrics endpoint: off by default. See `reference/config.md` for the
  current `profile.observability.metrics_enabled` status before enabling a
  metrics-feature build.
- Audit trail: when durable audit is enabled, the audit log lives
  under the managed state directory with the same mode
  enforcement as the vault.

## Firewall, SELinux, AppArmor, sandboxing

### Outbound network

The daemon makes **outbound-only** HTTPS connections (443) to
`*.pcloud.com` API endpoints. No inbound network surface. Egress
firewalls should allow:

```
TCP/443 → bineapi.pcloud.com
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

If audit2allow flags violations under the managed state directory,
prefer a user-type transition over relaxing the global policy.

### AppArmor (Debian / Ubuntu)

An in-tree starting profile exists at
`packaging/apparmor/usr.local.bin.pcloudd`. If you author or adapt one,
grant the current managed paths:

```
owner @{HOME}/.config/pcloud/pcloud-rs/** rw,
owner @{HOME}/.local/share/pcloud/pcloud-rs/** rw,
owner @{HOME}/.cache/pcloud/pcloud-rs/** rw,
/dev/fuse rw,
@{PROC}/@{pid}/mountinfo r,
@{run}/user/@{uid}/pcloud/pcloud-rs/** rw,
network inet stream,
```

### nftables / iptables

Host firewalls are not load-bearing because the daemon does not
listen on any IP socket in the default configuration. If you enable
the optional Prometheus metrics port, restrict it to `127.0.0.1`.

## Troubleshooting (top 10)

1. **`Connection refused` on `pcloudc`** — daemon not running.
   ```bash
   systemctl --user status pcloudd.service
   journalctl --user -u pcloudd.service -n 200 --no-pager
   ```
2. **`EACCES` on socket** — wrong UID or relaxed perms.
   ```bash
   ls -la "$XDG_RUNTIME_DIR/pcloud/pcloud-rs/"
   stat -c '%U:%G %a' "$XDG_RUNTIME_DIR/pcloud/pcloud-rs/pcloud.sock"
   ```
   Expected `0600`, owned by the invoking UID. Fix by restarting.
3. **`Vault rejected (mode 0644)`** — someone chmod'd the vault.
   ```bash
   chmod 0600 ~/.config/pcloud/pcloud-rs/auth_token
   chmod 0700 ~/.config/pcloud/pcloud-rs
   ```
4. **Mount hangs, `ls ~/pCloudDrive` blocks** — stale FUSE.
   ```bash
   fusermount3 -u -z ~/pCloudDrive
   PCLOUD_FORCE_UMOUNT=1 systemctl --user restart pcloudd.service
   ```
5. **`SQLITE_BUSY`** — state on NFS/CIFS. Move `XDG_DATA_HOME` to a
   local disk and restart.
6. **TLS errors to `bineapi.pcloud.com`** — missing CA bundle or MITM
   proxy. Verify:
   ```bash
   echo | openssl s_client -servername bineapi.pcloud.com \
     -connect bineapi.pcloud.com:443 >/dev/null
   ```
7. **`auth=NeedsTFA` on every start** — vault missing or not
   persisted.
   ```bash
   pcloudc auth status
   pcloudc login
   ```
8. **Daemon OOM-killed** — cap the unit.
   ```bash
   systemctl --user edit pcloudd.service
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
  the managed state directory before a major-version bump.
- Two-wave rolling strategy for managed fleets is documented in
  [Upgrade](../upgrade.md); the platform-specific hook here is that
  `systemctl --user daemon-reload` must run after swapping the user unit.

## Uninstalling

See the **Uninstall** section below for the step-by-step removal.

## Known gaps (Linux)

- No clean release-commit Linux qualification run or credentialed live-pCloud
  mount/transfer smoke result exists for the current tree.
- Produced `.deb` and `.rpm` candidates still need install, upgrade, service,
  mount, and uninstall verification on their target distributions.
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

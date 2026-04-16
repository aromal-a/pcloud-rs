# FreeBSD

Platform notes for running `pcloud-daemon` and `pcloud-cli` on FreeBSD
13.x and 14.x. Earlier releases are not supported; 14.x is the
preferred target because `fusefs-libs3` is a first-class port and the
kernel `fusefs(5)` implementation is mature.

## Support status

- **Scaffolded, not live-tested.** The Rust core builds cleanly on
  FreeBSD 14; FUSE mount, rc.d integration, and `pkg`/ports recipes
  are in tree but have not yet been exercised end-to-end on a real
  host.
- See
  [`architecture/platform-support.md`](../../architecture/platform-support.md).

## OS version matrix

| Release     | Arch            | Status                                |
|-------------|-----------------|---------------------------------------|
| 14.1        | amd64, arm64    | Build-only, no mount verification     |
| 14.0        | amd64, arm64    | Build-only                            |
| 13.3        | amd64           | Build-only — kernel FUSE lags         |
| 13.2        | amd64           | Build-only                            |
| <= 12.x     | any             | **Not supported**                     |

> **Landing status (2026-04-15):** Tier 2, wired, not yet live-verified
> on a FreeBSD host. Phases P0–P5 compile cleanly: `fuser` adapter over
> `fusefs-libs3`, rc.d unit, and a `pkg`/ports-ready manifest ship in
> tree. Host bring-up (mount lifecycle, rc.d start/stop, ports build)
> is still outstanding. BSD-specific issues should get their own beads.
> See [Packaging reference](../../reference/packaging.md).

## Install

### Packages / ports

```sh
# Binary package (pkg)
sudo pkg install pcloud-rs

# Ports (if you need to adjust build options)
cd /usr/ports/net/pcloud-rs
sudo make config-recursive install clean
```

Dependencies pulled in automatically:

- `fusefs-libs3` (FUSE userland),
- `ca_root_nss` (CA bundle for rustls),
- `sqlite3` (runtime dep).

`pcloudc` ships a built-in field selector (`--field PATH`, trailing
field names on read-only commands, `--json` for the full envelope) so
no external JSON post-processor is required for the runbook recipes.

### From source

```sh
sudo pkg install rust git fusefs-libs3 ca_root_nss pkgconf
git clone https://github.com/pcloud-rs/pcloud-rs
cd pcloud-rs/
cargo build --release -p pcloud-daemon -p pcloud-cli
sudo install -m 0755 target/release/pcloud-daemon /usr/local/sbin/pcloud-daemon
sudo install -m 0755 target/release/pcloud-cli    /usr/local/bin/pcloudc
```

### Verification

```sh
sha256 -c SHA256SUMS.txt
# If the release provides signify/minisign signatures:
signify -V -p pcloud-rs.pub -m SHA256SUMS.txt
```

## Config paths (XDG-style under `~/.config`)

FreeBSD honors XDG when the variables are set and falls back to
`~/.config/pcloud-rs` / `~/.local/share/pcloud-rs` / `~/.cache/pcloud-rs`
otherwise.

| Role          | Path                                              | Mode  |
|---------------|---------------------------------------------------|-------|
| Config        | `~/.config/pcloud-rs/config.toml`                   | 0600  |
| Store         | `~/.local/share/pcloud-rs/store.sqlite`             | 0600  |
| Vault         | `~/.local/share/pcloud-rs/vault.dat`                | 0600  |
| Journal       | `~/.local/share/pcloud-rs/journal/`                 | 0700  |
| Cache         | `~/.cache/pcloud-rs/`                               | 0700  |
| IPC socket    | `/var/run/user/$(id -u)/pcloud-rs/daemon.sock` (if `XDG_RUNTIME_DIR` is set) or `/tmp/pcloud-rs-$(id -u)/daemon.sock` (fallback) | 0600 |
| Log           | `/var/log/pcloud-rs/daemon.log` (system service) or `~/.local/state/pcloud-rs/daemon.log` (user run) | 0600 |

The package's rc.d script uses `/var/log/pcloud-rs/` owned by the
`pcloud-rs` user. If you run the daemon under your own user, it falls
back to the `~/.local/state` path.

Create per-user dirs with correct modes:

```sh
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs
```

## Service management (rc.d)

The package drops an rc.d script at `/usr/local/etc/rc.d/pcloudd`:

```sh
# Enable at boot
sudo sysrc pcloudd_enable="YES"

# Optional: run as a dedicated user created by the package
sudo sysrc pcloudd_user="pcloud-rs"
sudo sysrc pcloudd_group="pcloud-rs"

# Start / stop / restart / status
sudo service pcloudd start
sudo service pcloudd stop
sudo service pcloudd restart
sudo service pcloudd status
```

Script skeleton (installed by the package, shown here for reference):

```sh
#!/bin/sh
# PROVIDE: pcloudd
# REQUIRE: NETWORKING
# KEYWORD: shutdown
. /etc/rc.subr
name="pcloudd"
rcvar="pcloudd_enable"
command="/usr/local/sbin/pcloud-daemon"
command_args="--log-format json --log-level info \
  --config /usr/local/etc/pcloud-rs/config.toml"
pidfile="/var/run/pcloudd.pid"
load_rc_config $name
: ${pcloudd_enable:=NO}
: ${pcloudd_user:=pcloud-rs}
: ${pcloudd_group:=pcloud-rs}
run_rc_command "$1"
```

For per-user operation on a desktop, skip the rc.d script and launch
the daemon from the user's shell or an X/Wayland session-start hook;
XDG runtime paths are the same as on Linux.

## Mount setup (fusefs)

FreeBSD's kernel FUSE module (`fusefs.ko`) plus the `fusefs-libs3`
userspace port provide FUSE3 compatibility.

```sh
sudo pkg install fusefs-libs3
sudo kldload fusefs
sudo sysrc -f /etc/rc.conf.d/fusefs kld_list+="fusefs"
```

Verify:

```sh
fusermount3 --version    # from fusefs-libs3
kldstat | grep fusefs
```

Allow non-root users to mount (one of):

```sh
# System-wide
sudo sysctl vfs.usermount=1
echo 'vfs.usermount=1' | sudo tee -a /etc/sysctl.conf
```

Or run the daemon under a user in the `operator` group that already
has mount rights.

Configure the mount:

```toml
[mount]
enabled = true
path    = "/home/alice/pCloudDrive"
policy  = "default"
```

Wedged-mount recovery — see
[runbook.md Playbook 7](../runbook.md#playbook-7-kernel-mount-recovery):

```sh
sudo umount -f /home/alice/pCloudDrive
# then:
sudo service pcloudd restart
```

## Vault backend

File-backed vault at `~/.local/share/pcloud-rs/vault.dat`, mode `0600`,
parent `0700`, UID-bound. No system keyring integration on FreeBSD;
track the Keychain / Secret Service bead. In-memory secrets use
`SecretString` / `SecretBytes` with zeroize-on-drop.

For disk-level protection, GELI full-disk encryption on the vault's
filesystem provides the equivalent of LUKS.

## Upgrade

See [Upgrade](../upgrade.md). Quick path:

```sh
pcloudc --json status > /tmp/pre.json
sudo service pcloudd stop
sudo pkg upgrade pcloud-rs
sudo service pcloudd start
pcloudc doctor --json
pcloudc status              # auth=Authenticated, healthy engine summary
```

For ports-based installs, rebuild and reinstall via
`make deinstall install clean` and restart the service.

## Uninstall

```sh
# 1. Stop and disable
sudo service pcloudd stop
sudo sysrc -x pcloudd_enable

# 2. Remove the package
sudo pkg delete pcloud-rs

# 3. Remove per-user state (this deletes the vault)
rm -rf \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.cache/pcloud-rs

# 4. Remove system log dir if unused
sudo rm -rf /var/log/pcloud-rs
sudo rm -rf /usr/local/etc/pcloud-rs

# 5. Remove the dedicated user (only if the package created it)
sudo pw userdel pcloud-rs || true
sudo pw groupdel pcloud-rs || true
```

Verify:

```sh
pgrep -lf pcloud-daemon || echo "clean"
mount | grep pcloud || echo "clean"
```

## First-run bootstrap

Beginner path:

```sh
# 1. Load kernel FUSE module at boot
sudo kldload fusefs
sudo sysrc -f /etc/rc.conf.d/fusefs kld_list+="fusefs"

# 2. Allow non-root mounts (only if you plan to run per-user)
sudo sysctl vfs.usermount=1
echo 'vfs.usermount=1' | sudo tee -a /etc/sysctl.conf

# 3. Create state dirs
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs

# 4. Enable and start the rc.d service
sudo sysrc pcloudd_enable="YES"
sudo service pcloudd start

# 5. Sanity check
pcloudc doctor --json
pcloudc status
```

FAANG-ops tuning callouts:

- Run the daemon inside a thin jail with `allow.mount.fusefs=1` and
  a `devfs.rules` ruleset exposing only `/dev/fuse`. This is the
  correct isolation primitive on FreeBSD.
- Use ZFS boot environments (`bectl`) when upgrading in production;
  `bectl create pre-pcloud-rs-<version>` gives instant rollback.

## Service management cheat-sheet

| Action          | Command                                     |
|-----------------|---------------------------------------------|
| Start           | `sudo service pcloudd start`                |
| Stop            | `sudo service pcloudd stop`                 |
| Restart         | `sudo service pcloudd restart`              |
| Status          | `sudo service pcloudd status`               |
| Enable at boot  | `sudo sysrc pcloudd_enable="YES"`           |
| Disable at boot | `sudo sysrc -x pcloudd_enable`              |
| Tail log        | `sudo tail -f /var/log/pcloud-rs/daemon.log` |
| Core dump       | `/var/crash/pcloud-rs/` (if configured)      |

Enable core dumps:

```sh
sudo sysctl kern.corefile=/var/crash/%N.core.%P
sudo sysctl kern.coredump=1
```

## Peer-cred and IPC

- Transport: `AF_UNIX` stream socket. Default path is
  `/var/run/user/$(id -u)/pcloud-rs/daemon.sock` when
  `XDG_RUNTIME_DIR` is set by `pam_xdg` (or similar), otherwise
  `/tmp/pcloud-rs-$(id -u)/daemon.sock`, both `0600` with a `0700`
  parent.
- Peer identity: the daemon uses `getpeereid(2)`, which is the
  FreeBSD native. UID mismatch rejects the connection.
- `SO_PEERCRED` exists on FreeBSD but is not the canonical API here;
  we keep a single BSD code path using `getpeereid`.

## Secret storage backend

- In-memory: `SecretString` / `SecretBytes` (zeroize-on-drop).
- On-disk: file-backed vault at
  `~/.local/share/pcloud-rs/vault.dat`, mode `0600`, parent `0700`,
  UID-bound.
- No system keyring (no `gnome-keyring` default, no KWallet on
  FreeBSD out of the box).
- Disk-level protection: GELI full-disk encryption on the vault
  filesystem; the vault inherits the underlying protection.

## Observability integration

- `syslog(3)` is used for ERROR-level events; the JSON structured
  log is written to `/var/log/pcloud-rs/daemon.log` when running as a
  system service.
- `newsyslog` rotation is configured by the package under
  `/etc/newsyslog.conf.d/pcloud-rs.conf`.
- DTrace: no project-specific USDT probes yet. Use `dtrace -p <pid>
  -n 'syscall::read:entry /pid == $target/ { @[execname] = count() }'`
  for generic tracing.

## Firewall / jails / MAC

- **pf** rules need egress 443 to the pCloud API endpoints.
- **Jails**: require `allow.mount`, `allow.mount.fusefs`, and a
  `devfs.rules` entry exposing `/dev/fuse`. Without them the daemon
  starts but `mount` fails cleanly; no silent degradation.
- **MAC policies** (`mac_bsdextended`, `mac_portacl`): no opinions
  required — the daemon does not open listening sockets.

## Troubleshooting (top 10)

1. **`mount: Operation not permitted`** — `vfs.usermount` not set
   or user not in the `operator` group.
2. **`fusefs.ko` not loaded** — `kldload fusefs`. Persist via
   `/etc/rc.conf.d/fusefs`.
3. **`pkg install pcloud-rs` fails** — repo not trusted; verify:
   ```sh
   pkg -vv | grep -i pubkey
   ```
4. **ZFS snapshot blocks vault open** — expected; vault opens are
   UID + mode strict.
5. **TLS failures** — `ca_root_nss` missing:
   `sudo pkg install ca_root_nss`.
6. **Jail mount fails** — missing `allow.mount.fusefs` or
   `devfs.rules`. `jls -v` to inspect current params.
7. **`service pcloudd status` shows running but CLI can't connect**
   — `$XDG_RUNTIME_DIR` mismatch between shell and service. Use
   `sockstat -u | grep pcloud` to find the real socket path.
8. **Rust build OOMs** — bump `MAKEFLAGS="-j1"` or build in tmpfs
   with `MAKEOBJDIR` on a larger disk.
9. **13.x `EPROTO` on readdir** — kernel FUSE lag; upgrade to 14.x
   when possible.
10. **Jail clock drift** — `ntpd` inside the jail or synced host
    clock via `host_bindings`.

## Upgrading

- `pkg upgrade pcloud-rs`; restart via `service pcloudd restart`.
- For ports: `make deinstall && make install clean`.

## Uninstalling

See the **Uninstall** section below.

## Known gaps (FreeBSD)

- No Capsicum `cap_enter(2)` path.
- No DTrace USDT provider.
- No Linuxulator support.
- Kernel FUSE protocol on 13.x is behind the userspace version.

## Known issues

- **Capsicum sandbox.** The daemon does not currently enter capability
  mode on FreeBSD. Track the follow-up bead for `cap_enter(2)` support
  if you need stricter confinement.
- **Jails.** Running inside a jail requires `allow.mount.fusefs=1` in
  the jail config plus `devfs.rules` that exposes `/dev/fuse`. Without
  these, the daemon starts but `mount` fails cleanly. No silent
  degradation.
- **ZFS on the vault.** ZFS snapshots of the vault work but remember
  the vault is UID-bound — restoring a snapshot to a different UID
  (e.g., via `zfs send` to another host) yields a rejected vault, by
  design.
- **DTrace audit.** No DTrace probes are registered yet. Use the
  structured JSON log (`/var/log/pcloud-rs/daemon.log`) for forensics.
- **Linuxulator.** Running the Linux binary under `linux` emulation is
  not supported — use the native FreeBSD build.
- **Kernel FUSE vs fuse-libs3 skew.** On 13.x the kernel FUSE lags the
  userspace protocol; prefer 14.x. If you must run 13.x, expect
  occasional `EPROTO` on readdir that the daemon retries transparently
  but logs as `fs.readdir.retried`.

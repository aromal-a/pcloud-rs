# NetBSD

Platform notes for running `pcloud-daemon` and `pcloud-cli` on NetBSD
9.x and 10.x. NetBSD is a **best-effort** target: the Rust core builds
and runs, and FUSE via `refuse` / `puffs` is usable, but mount-layer
coverage and tooling are less mature than on Linux or FreeBSD.

## Support status

- **Best-effort, scaffolded.** Build compiles against the `refuse` /
  `puffs` shim; mount lifecycle is experimental. No live host
  verification.
- Truth source:
  [`architecture/platform-support.md`](../../architecture/platform-support.md).

## OS version matrix

| Release | Arch                     | Status                                |
|---------|--------------------------|---------------------------------------|
| 10.0    | amd64, arm64             | Build-only, best-effort               |
| 9.3     | amd64                    | Build-only, best-effort               |
| 9.2     | amd64                    | Builds but known `refuse` gaps        |
| <= 8.x  | any                      | **Not supported**                     |

> **Landing status (2026-04-15):** Tier 3 best-effort. P0–P5 traits
> compile against the `refuse` / `puffs` shim and an rc.d script ships
> under `packaging/netbsd/`. Mount lifecycle is experimental; no live
> host verification is expected at this tier. File a dedicated NetBSD
> bead for anything platform-specific. See
> [Packaging reference](../../reference/packaging.md).

## Install

### pkgsrc

```sh
# Binary package (recommended)
sudo pkgin install pcloud-rs

# Building from pkgsrc
cd /usr/pkgsrc/net/pcloud-rs
sudo make install clean clean-depends
```

Dependencies pulled in automatically:

- `rust` (if building from source),
- `fuse` (via pkgsrc, provides `libfuse` + `refuse`/`puffs` glue),
- `mozilla-rootcerts-openssl` (CA bundle for rustls),
- `sqlite3`.

### From source

```sh
sudo pkgin install rust git fuse mozilla-rootcerts-openssl pkgconf
git clone https://github.com/pcloud-rs/pcloud-rs
cd pcloud-rs/
cargo build --release -p pcloud-daemon -p pcloud-cli
sudo install -m 0755 target/release/pcloud-daemon /usr/pkg/sbin/pcloud-daemon
sudo install -m 0755 target/release/pcloud-cli    /usr/pkg/bin/pcloudc
```

Make sure `mozilla-rootcerts-openssl` has been run:

```sh
sudo mozilla-rootcerts install
```

### Verification

```sh
cksum -a sha256 pcloud-daemon
signify -V -p pcloud-rs.pub -m SHA256SUMS.txt
```

## Config paths

NetBSD honors XDG when set; pkgsrc-installed daemons default to
`~/.config/pcloud-rs`, `~/.local/share/pcloud-rs`, and
`~/.cache/pcloud-rs`. The IPC socket location defaults to `/tmp` since
`/var/run/user` does not exist by convention.

| Role          | Path                                                                 | Mode  |
|---------------|----------------------------------------------------------------------|-------|
| Config        | `~/.config/pcloud-rs/config.toml`                                     | 0600  |
| Store         | `~/.local/share/pcloud-rs/store.sqlite`                               | 0600  |
| Vault         | `~/.local/share/pcloud-rs/vault.dat`                                  | 0600  |
| Journal       | `~/.local/share/pcloud-rs/journal/`                                   | 0700  |
| Cache         | `~/.cache/pcloud-rs/`                                                 | 0700  |
| IPC socket    | `/tmp/pcloud-rs-$(id -u)/daemon.sock` (or `$XDG_RUNTIME_DIR` if set)  | 0600  |
| Log           | `/var/log/pcloud-rs/daemon.log` (system) or `~/.local/state/pcloud-rs/daemon.log` (user) | 0600 |

System-wide config for the pkgsrc service lives at
`/usr/pkg/etc/pcloud-rs/config.toml` and is loaded when the daemon is
launched from `rc.d` as the `pcloud-rs` user.

Create per-user dirs once:

```sh
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs
```

## Service management (rc.d)

The pkgsrc package installs an rc.d script at
`/etc/rc.d/pcloudd` (or `/usr/pkg/share/examples/rc.d/pcloudd` that
you copy into `/etc/rc.d/`):

```sh
# Enable at boot
echo 'pcloudd=YES' | sudo tee -a /etc/rc.conf

# Start/stop
sudo /etc/rc.d/pcloudd start
sudo /etc/rc.d/pcloudd stop
sudo /etc/rc.d/pcloudd status
sudo /etc/rc.d/pcloudd restart
```

Skeleton:

```sh
#!/bin/sh
# PROVIDE: pcloudd
# REQUIRE: NETWORKING
# KEYWORD: shutdown

$_rc_subr_loaded . /etc/rc.subr

name="pcloudd"
rcvar=$name
command="/usr/pkg/sbin/pcloud-daemon"
command_args="--log-format json --log-level info \
  --config /usr/pkg/etc/pcloud-rs/config.toml"
pidfile="/var/run/${name}.pid"
required_files="/usr/pkg/etc/pcloud-rs/config.toml"
start_precmd="pcloudd_prestart"

pcloudd_prestart() {
  install -d -m 0755 -o pcloud-rs -g pcloud-rs /var/run
  install -d -m 0700 -o pcloud-rs -g pcloud-rs /var/log/pcloud-rs
}

load_rc_config $name
run_rc_command "$1"
```

For desktop use, launch the daemon from your `.xinitrc` or shell init
under your own UID; the state paths above work identically.

## Mount setup (`refuse` / `puffs`)

NetBSD implements FUSE via two interlocking pieces:

- `puffs(3)` — the kernel's userspace filesystem framework,
- `refuse(3)` — a FUSE-compatibility shim over `puffs`.

Both are part of the base system on 9.x+; the `fuse` pkgsrc package
provides the userland tooling that calls into them.

```sh
sudo pkgin install fuse
```

Verify:

```sh
grep puffs /etc/rc.d/* 2>/dev/null
sysctl vfs.generic | grep -i puffs
```

Allow non-root mounts:

```sh
sudo sysctl -w vfs.generic.usermount=1
echo 'vfs.generic.usermount=1' | sudo tee -a /etc/sysctl.conf
```

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
sudo /etc/rc.d/pcloudd restart
```

Note: `refuse` does not implement the full FUSE3 protocol. A small
subset of FUSE operations (notify invalidate, some readdirplus flags)
are handled as no-ops; the daemon detects this at attach and logs
`fs.kernel.caps.reduced`, then degrades gracefully.

## Vault backend

File-backed vault at `~/.local/share/pcloud-rs/vault.dat`, mode `0600`,
parent `0700`, UID-bound. No system-keyring integration. In-memory
secrets use `SecretString` / `SecretBytes` with zeroize-on-drop.

For disk-level protection, use `cgd(4)` (cryptographic disk device) on
the vault's filesystem; the vault inherits the protection.

## Upgrade

See [Upgrade](../upgrade.md). Quick path:

```sh
pcloudc --json status > /tmp/pre.json
sudo /etc/rc.d/pcloudd stop
sudo pkgin upgrade pcloud-rs
sudo /etc/rc.d/pcloudd start
pcloudc doctor --json
pcloudc status              # auth=Authenticated, healthy engine summary
```

## Uninstall

```sh
# 1. Stop and disable
sudo /etc/rc.d/pcloudd stop
sudo sed -i '' '/^pcloudd=/d' /etc/rc.conf

# 2. Remove the package
sudo pkgin remove pcloud-rs
# or, under pkgsrc directly:
# cd /usr/pkgsrc/net/pcloud-rs && sudo make deinstall

# 3. Remove per-user state (this deletes the vault)
rm -rf \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.cache/pcloud-rs

# 4. Remove system paths if unused
sudo rm -rf /var/log/pcloud-rs /usr/pkg/etc/pcloud-rs

# 5. Remove the dedicated user (only if the package created it)
sudo userdel pcloud-rs || true
sudo groupdel pcloud-rs || true
```

Verify:

```sh
pgrep -lf pcloud-daemon || echo "clean"
mount | grep pcloud || echo "clean"
```

## First-run bootstrap

```sh
# 1. Allow non-root mounts
sudo sysctl -w vfs.generic.usermount=1
echo 'vfs.generic.usermount=1' | sudo tee -a /etc/sysctl.conf

# 2. Create state dirs
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs

# 3. Enable the rc.d service
echo 'pcloudd=YES' | sudo tee -a /etc/rc.conf
sudo /etc/rc.d/pcloudd start

# 4. Sanity check
pcloudc doctor --json
pcloudc status
```

FAANG-ops tuning callouts:

- NetBSD's `refuse` / `puffs` stack is protocol-reduced vs Linux
  FUSE3; set `mount.retry_on_kernel_caps_reduced=true` and watch for
  `fs.kernel.caps.reduced` in the log.
- Use `veriexec` to fingerprint the daemon binary for integrity
  enforcement.

## Peer-cred and IPC

- Transport: `AF_UNIX` at `/tmp/pcloud-rs-$(id -u)/daemon.sock`, `0600`
  + `0700`-parent.
- Peer identity: `getpeereid(2)`.
- No `SO_PEERCRED` available.

## Secret storage backend

- In-memory: `SecretString` / `SecretBytes`.
- On-disk: file-backed vault, same posture as the other BSDs.
- Disk-level: `cgd(4)` on the vault filesystem.
- No system keyring.

## Observability

- `syslog(3)` via `LOG_DAEMON` plus JSON log at
  `/var/log/pcloud-rs/daemon.log`.
- `blocklistd` is optional — not used by the daemon.
- `ktruss`, `ktrace`, and `kdump` are the tracing primitives.

## Firewall

- **npf** / **pf** rules must allow outbound 443.
- No inbound rules required.

## Troubleshooting (top 10)

1. **`refuse` missing ops** — `fs.kernel.caps.reduced` in log. Not
   an error unless you see data inconsistency.
2. **`mozilla-rootcerts` not installed** — TLS failures. Run
   `sudo mozilla-rootcerts install`.
3. **`rust` from pkgsrc lags MSRV** — install `rustup` and pin a
   stable toolchain.
4. **Mount wedges on heavy write** — tune
   `mount.write_concurrency=2`.
5. **`vfs.generic.usermount=0`** — set it as shown above.
6. **No `/run/user`** — same fallback as OpenBSD.
7. **Upgrade clobbers vault perms** — verify `stat` after pkg
   upgrade; chmod back to `0600` if needed.
8. **sparc/m68k builds fail** — unsupported architectures.
9. **`pcloudd` pidfile stale** — `/var/run/pcloudd.pid` left behind
   after a crash. Remove and restart.
10. **Clock drift** — enable `ntpd_enable=YES`.

## Upgrading

- `sudo pkgin upgrade pcloud-rs` and restart.
- pkgsrc source: `make deinstall && make install clean`.

## Uninstalling

See the **Uninstall** section below.

## Known gaps (NetBSD)

- No system keyring.
- `refuse`/`puffs` protocol gaps (see log for caps-reduced events).
- No pledge/unveil analogue.
- Only amd64 and arm64 get routine coverage.

## Known issues

- **`refuse` / `puffs` protocol gaps.** NetBSD's FUSE-compat layer
  lacks newer FUSE3 operations. The daemon downgrades cleanly but you
  will see `fs.kernel.caps.reduced` warnings in the log. Do not treat
  these as errors; do file beads if they correlate with visible data
  inconsistencies.
- **No systemd / no `/run/user`.** Scripts from the Linux chapter that
  assume `$XDG_RUNTIME_DIR` must be adapted; the daemon defaults to
  `/tmp/pcloud-rs-$UID/`.
- **Rust toolchain churn.** pkgsrc's Rust occasionally lags the MSRV
  of the Rust workspace. If the package fails to build, install
  `rustup` and use a pinned stable toolchain.
- **Audit framework.** NetBSD does not ship `auditd`; rely on the
  JSON log for forensics.
- **ZFS on NetBSD.** Works but is experimental. Vault on ZFS is
  supported; snapshot + `zfs send` cross-UID is rejected by the
  daemon, as on every other platform.
- **Architecture coverage.** x86_64 and arm64 are tested. sparc64,
  m68k, and other lesser tier-2 architectures compile but are not
  routinely exercised — file beads generously.
- **No `pledge` / `unveil`.** The OpenBSD sandboxing is not available
  here. Rely on file permissions and the dedicated `pcloud-rs` user
  instead.

# OpenBSD

Platform notes for running `pcloud-daemon` and `pcloud-cli` on OpenBSD
7.3+. OpenBSD is a **best-effort** target: the Rust core compiles and
runs, but the platform's conservative stance on FUSE means mount
support is more restricted than on Linux or FreeBSD.

## Support status

- **Best-effort, scaffolded.** CLI and daemon compile; FUSE read path
  works via `libfuse`; write path is experimental because the OpenBSD
  kernel's FUSE is read-oriented.
- Authoritative matrix:
  [`architecture/platform-support.md`](../../architecture/platform-support.md).

## OS version matrix

| Release      | Arch         | Status                          |
|--------------|--------------|---------------------------------|
| 7.4, 7.5     | amd64, arm64 | Build-only, best-effort         |
| 7.3          | amd64        | Build-only, best-effort         |
| <= 7.2       | any          | **Not supported**               |

> **Landing status (2026-04-15):** Tier 3 best-effort. P0–P5 traits and
> read-path callbacks compile on OpenBSD with `libfuse`, and an rc.d
> script ships under `packaging/openbsd/`. The kernel's read-heavy
> `fuse(4)` constrains write behaviour; treat writeback as experimental
> regardless of host verification state. See
> [Packaging reference](../../reference/packaging.md).

## Install

### Packages

```sh
doas pkg_add pcloud-rs
```

Dependencies pulled in automatically:

- `rust` (if building),
- `libfuse` (OpenBSD's FUSE userland),
- root CA bundle from the base system (`/etc/ssl/cert.pem`).

### From source

```sh
doas pkg_add rust git libfuse
git clone https://github.com/pcloud-rs/pcloud-rs
cd pcloud-rs/
cargo build --release -p pcloud-daemon -p pcloud-cli
doas install -m 0755 target/release/pcloud-daemon /usr/local/sbin/pcloud-daemon
doas install -m 0755 target/release/pcloud-cli    /usr/local/bin/pcloudc
```

### Verification

```sh
sha256 -c SHA256SUMS.txt
signify -V -p pcloud-rs.pub -m SHA256SUMS.txt
```

`signify(1)` is the OpenBSD-native signature-verification tool; prefer
it over PGP/Cosign where a release key ships in both formats.

## Config paths

OpenBSD honors XDG when set and falls back to the same `~/.config`
tree as Linux. The pattern is identical — only the default IPC socket
location differs.

| Role          | Path                                                                 | Mode  |
|---------------|----------------------------------------------------------------------|-------|
| Config        | `~/.config/pcloud-rs/config.toml`                                     | 0600  |
| Store         | `~/.local/share/pcloud-rs/store.sqlite`                               | 0600  |
| Vault         | `~/.local/share/pcloud-rs/vault.dat`                                  | 0600  |
| Journal       | `~/.local/share/pcloud-rs/journal/`                                   | 0700  |
| Cache         | `~/.cache/pcloud-rs/`                                                 | 0700  |
| IPC socket    | `/tmp/pcloud-rs-$(id -u)/daemon.sock` (OpenBSD has no `/run/user`)    | 0600  |
| Log           | `/var/log/pcloud-rs/daemon.log` (system) or `~/.local/state/pcloud-rs/daemon.log` (user) | 0600 |

Create per-user dirs once:

```sh
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs
```

The IPC socket's parent directory (`/tmp/pcloud-rs-$(id -u)/`) is
created `0700` at daemon startup and rechecked on every bind. If the
daemon finds the directory with a relaxed mode, it refuses to bind
rather than trust it.

## Service management (rc.d)

The package installs `/etc/rc.d/pcloudd`:

```sh
# Enable at boot
doas rcctl enable pcloudd

# Optional: dedicated user created by the package
doas rcctl set pcloudd user pcloud-rs

# Start/stop/status
doas rcctl start pcloudd
doas rcctl stop  pcloudd
doas rcctl check pcloudd
```

Skeleton (shipped by the port):

```sh
#!/bin/ksh
daemon="/usr/local/sbin/pcloud-daemon"
daemon_flags="--log-format json --log-level info \
  --config /etc/pcloud-rs/config.toml"
daemon_user="pcloud-rs"

. /etc/rc.d/rc.subr

rc_bg=YES
rc_reload=NO

rc_cmd $1
```

For interactive desktop use, run the daemon from your `~/.xsession`
or shell-init scripts as your own user; the state paths above are
fully honored in either mode.

## Mount setup (FUSE)

OpenBSD provides FUSE via the `libfuse` package and the `fuse(4)`
kernel device. Mount support is **read-oriented** on OpenBSD — writes
work but performance and crash recovery are not on par with Linux or
FreeBSD, and `mount -t fuse` is subject to `kern.usermount` policy.

```sh
doas pkg_add libfuse
doas sysctl kern.usermount=1   # runtime
echo 'kern.usermount=1' | doas tee -a /etc/sysctl.conf
```

Verify:

```sh
fusermount --version
sysctl kern.usermount
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
doas umount -f /home/alice/pCloudDrive
doas rcctl restart pcloudd
```

OpenBSD does not ship `fusermount3`; `fusermount` + `umount -f` is the
recovery path.

## Vault backend

File-backed vault at `~/.local/share/pcloud-rs/vault.dat`, mode `0600`,
parent `0700`, UID-bound. OpenBSD has no system keyring; the vault's
protection is entirely filesystem-based plus the daemon's in-process
`SecretString` / `SecretBytes` wrappers (zeroize-on-drop).

For disk-level protection, use softraid(4) with CRYPTO or a RAID
discipline; the vault inherits the underlying protection.

The daemon uses `pledge(2)` to drop privileges after startup on
OpenBSD. The pledge set is minimal: `"stdio rpath wpath cpath unix
fattr inet dns tty"` during startup, narrowed to `"stdio rpath wpath
cpath unix inet dns"` once initialization completes. Do not patch the
pledge set unless you have a bead explaining why.

## Upgrade

See [Upgrade](../upgrade.md). Quick path:

```sh
pcloudc --json status > /tmp/pre.json
doas rcctl stop pcloudd
doas pkg_add -u pcloud-rs
doas rcctl start pcloudd
pcloudc doctor --json
pcloudc status              # auth=Authenticated, healthy engine summary
```

## Uninstall

```sh
# 1. Stop and disable
doas rcctl stop pcloudd
doas rcctl disable pcloudd

# 2. Remove the package
doas pkg_delete pcloud-rs

# 3. Remove per-user state (this deletes the vault)
rm -rf \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.cache/pcloud-rs

# 4. Remove system paths if unused
doas rm -rf /var/log/pcloud-rs /etc/pcloud-rs

# 5. Remove the dedicated user (only if the package created it)
doas userdel pcloud-rs || true
doas groupdel pcloud-rs || true
```

Verify:

```sh
pgrep -lf pcloud-daemon || echo "clean"
mount | grep pcloud || echo "clean"
```

## First-run bootstrap

```sh
# 1. Allow non-root mounts
doas sysctl kern.usermount=1
echo 'kern.usermount=1' | doas tee -a /etc/sysctl.conf

# 2. Create state dirs
install -d -m 0700 \
  ~/.config/pcloud-rs \
  ~/.local/share/pcloud-rs \
  ~/.local/share/pcloud-rs/journal \
  ~/.cache/pcloud-rs

# 3. Enable and start the rc.d service
doas rcctl enable pcloudd
doas rcctl start pcloudd

# 4. Sanity check
pcloudc doctor --json
pcloudc status
```

FAANG-ops tuning callouts:

- OpenBSD is a natural fit for a bastion or security-hardened workstation.
  Keep the daemon under its default `pledge(2)` / `unveil(2)` policy;
  do not widen either without an explicit bead.
- `sysupgrade` handling: stop `pcloudd` before `sysupgrade`, restart
  after first reboot into the new release.

## Peer-cred and IPC

- Transport: `AF_UNIX` at `/tmp/pcloud-rs-$(id -u)/daemon.sock`, mode
  `0600`, parent `0700`. OpenBSD has no `/run/user` convention.
- Peer identity: `getpeereid(2)` + UID compare. OpenBSD does not
  expose `SO_PEERCRED`.
- The daemon `unveil`s only the per-user state dir and the sync-root
  paths, so even root cannot read the vault from an unrelated path
  at runtime.

## Secret storage backend

- In-memory: `SecretString` / `SecretBytes`.
- On-disk: file-backed vault at `~/.local/share/pcloud-rs/vault.dat`,
  `0600` + `0700`-parent + UID-bound, re-verified on every open.
- Disk-level: softraid(4) CRYPTO / cgd-style encryption on the vault
  filesystem.
- No system keyring.

## Observability

- `syslog(3)` via `LOG_DAEMON` by default; `/etc/syslog.conf` routes
  the `daemon.info` stream to `/var/log/daemon` or, when enabled,
  `/var/log/pcloud-rs/daemon.log`.
- Structured JSON log is also written to the same file.
- OpenBSD has no auditd; rely on the JSON log for forensics.

## Firewall / pledge / unveil

- **pf** rules must allow outbound 443.
- **pledge(2)** set: `"stdio rpath wpath cpath unix fattr inet dns
  tty"` at startup, narrowed to `"stdio rpath wpath cpath unix inet
  dns"` after initialization. A pledge violation is **fatal** — do
  not paper over abort traps; capture `ktrace` and file a bead.
- **unveil(2)**: the daemon re-opens unveil when a sync root is added;
  if you see `ENOENT` right after registering a new root, restart the
  daemon.

## Troubleshooting (top 10)

1. **`mount: Operation not permitted`** — `kern.usermount` off.
2. **Pledge abort on shutdown** — missing syscall in the shutdown-path
   pledge set. Capture `ktrace -f /tmp/pcloudd.kt -p $(pgrep pcloud-daemon)`.
3. **Unveil `ENOENT` after sync-root add** — daemon restart required.
4. **`fusermount` missing** — install `libfuse`; OpenBSD has no
   `fusermount3`.
5. **TLS failures** — ensure `/etc/ssl/cert.pem` exists
   (`make install` from `ports/security/mozilla-rootcerts` if not).
6. **`rcctl start pcloudd` fails silently** — check
   `/var/log/messages`; pledge violation shows as `abort trap`.
7. **Write-heavy sync-root wedges** — expected on OpenBSD FUSE;
   reduce `mount.write_concurrency` to `2`.
8. **doas vs sudo** — use `doas`; examples from the Linux chapter
   using `sudo` must be translated.
9. **No `/run/user`** — paths in cross-platform scripts must fall
   back to `/tmp/pcloud-rs-$UID`.
10. **CVS/git cert verification against GitHub** — install
    `mozilla-rootcerts` as above.

## Upgrading

- Release upgrades: `sysupgrade` (stop daemon first, restart after).
- Package upgrades: `doas pkg_add -u pcloud-rs`.

## Uninstalling

See the **Uninstall** section below.

## Known gaps (OpenBSD)

- No keyring.
- No write-heavy workload guarantee.
- No fusermount3 (`fusermount` only).
- Only read path is considered stable.

## Known issues

- **Write performance.** OpenBSD's FUSE is optimized for read-heavy
  workloads. Large write-back queues will log
  `fs.writeback.backpressure` more often than on Linux. Tune
  `mount.write_concurrency` conservatively (2-4).
- **No `/run/user`.** The IPC socket lives under `/tmp/pcloud-rs-$UID/`
  instead of `$XDG_RUNTIME_DIR`. Your scripts must handle both paths
  if they are cross-platform.
- **`pledge(2)` tightening.** A regression surfacing as
  `abort trap` at shutdown is almost always a missed pledge; capture
  `ktrace` and file a bead — do not widen the pledge set.
- **`unveil(2)` coverage.** The daemon uses `unveil(2)` to restrict
  filesystem access to the state + sync-root paths. If you register a
  new sync root after startup, the daemon re-opens unveil; failures
  present as `ENOENT` to the CLI. Restart the daemon if you see this
  after an unusual upgrade path.
- **No DPAPI / Keychain.** Vault is file-backed only.
- **smpr / doas vs sudo.** The runbook examples use `doas`; OpenBSD
  does not ship sudo. Substitute `doas` for `sudo` in any imported
  procedure.
- **Kernel FUSE protocol version.** OpenBSD's FUSE protocol is behind
  Linux's. A small subset of FUSE ops the daemon emits are handled as
  no-ops — the daemon detects this at mount-attach and logs
  `fs.kernel.caps.reduced`. Expect slightly weaker consistency
  semantics; file beads if you observe lost writes.

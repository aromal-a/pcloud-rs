# Deployment Guide (End-to-End Install)

This chapter is a **single-host install walkthrough** per platform.
Every command is runnable. For fleet rollout, multi-host topology, and
upgrade strategy, see the longer-form [Deployment](./deployment.md)
chapter. For platform-specific deep-dives (config paths, vault
backend, service-management cheat-sheets, troubleshooting top-10), see
the [`platforms/`](./platforms/linux.md) chapters.

> **Honesty callout.** The Rust rewrite is **not** a "drop-in
> replacement" for the legacy C client and has no public release. Linux is the
> Tier-1 reference target and has local kernel-adapter evidence, but it has not
> passed a clean, credentialed, installed-package release gate. macOS, Windows,
> every BSD/Unix target, and Tier-2 NAS packages still require their native
> qualification evidence. Do not deploy beyond the matrix in
> [`architecture/platform-support.md`](../architecture/platform-support.md).

---

## 1. Linux (Tier 1)

### 1.1 Prerequisites

- A 64-bit kernel, 4.18+.
- libfuse3 user-space helpers (`fuse3` package).
- `fusermount3` available on `PATH` and (for unprivileged mount of a
  pre-existing FUSE filesystem) the setuid bit set by the distro
  package.
- `/etc/fuse.conf` containing `user_allow_other` if and only if the
  mount must be visible to other UIDs (default: not required).
- `systemd` 245+ for the shipped unit. Older systemd may need
  back-ported directives stripped.

```bash
sudo apt-get install fuse3   # Debian/Ubuntu
sudo dnf     install fuse3   # Fedora/RHEL
sudo pacman  -S      fuse3   # Arch
```

### 1.2 Install the binaries

No project package repository or release tarball exists. Build the reviewed
checkout with the locked dependency graph:

```bash
git clone https://github.com/ezechiel203/pcloud-rs
cd pcloud-rs
cargo build --release --locked -p pcloud-daemon -p pcloud-cli
sudo install -Dm0755 target/release/pcloudd /usr/bin/pcloudd
sudo install -Dm0755 target/release/pcloudc /usr/bin/pcloudc
```

### 1.3 Install the systemd unit

> **Network model.** The shipped unit does **not** set `IPAddressDeny=` or
> `IPAddressAllow=` by default. Outbound API access is governed by the host
> firewall. Install `override.conf.example` only if you want an opt-in
> cgroup-level egress allow-list.

```bash
sudo install -Dm0644 \
  packaging/systemd/pcloudd.service \
  /etc/systemd/system/pcloudd.service

sudo systemctl daemon-reload
sudo systemctl enable --now pcloudd.service

# Optional: add a strict systemd egress allow-list on top of the host firewall.
sudo install -Dm0644 \
  packaging/systemd/override.conf.example \
  /etc/systemd/system/pcloudd.service.d/egress-allow-list.conf
sudo systemctl daemon-reload
sudo systemctl restart pcloudd.service
```

For a per-user deployment instead:

Use the dedicated user unit. Do not copy `pcloudd.service` into
`~/.config/systemd/user/`; it is system-only and contains directives
that user managers reject.

```bash
install -Dm0644 \
  packaging/systemd/pcloudd-user.service \
  ~/.config/systemd/user/pcloudd.service

systemctl --user daemon-reload
systemctl --user enable --now pcloudd.service

# Optional: add a strict systemd egress allow-list on top of the host firewall.
install -Dm0644 \
  packaging/systemd/override.conf.example \
  ~/.config/systemd/user/pcloudd.service.d/egress-allow-list.conf
systemctl --user daemon-reload
systemctl --user restart pcloudd.service
```

The shipped system unit uses `DynamicUser=yes` by default (ephemeral
UID, no home dir). To pin a fixed user in the system unit, see
[`packaging/systemd/override.conf.example`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/override.conf.example).

For non-interactive bootstrap, prefer systemd credentials over environment
secrets:

```ini
[Service]
LoadCredentialEncrypted=pcloud-rs-token:/etc/credstore.encrypted/pcloud-rs-token
```

The daemon reads `$CREDENTIALS_DIRECTORY/pcloud-rs-token` as a fallback for
`PCLOUDRS_TOKEN_FILE`, and similarly supports `pcloud-rs-username`,
`pcloud-rs-password`, `pcloud-rs-tfa-code`, and
`pcloud-rs-recovery-code`. Credential files must be regular files owned by
the daemon UID with no group/other permission bits.

### 1.4 Enable the FUSE override (only if mounting)

The shipped unit blocks `/dev/fuse` and the `@mount` syscall group by
default. Mount-using deployments must install the FUSE drop-in:

```bash
sudo systemctl edit pcloudd.service
# paste the contents of:
#   packaging/systemd/override-fuse.conf.example
sudo systemctl daemon-reload
sudo systemctl restart pcloudd.service
```

### 1.5 Verify the install

```bash
systemctl status pcloudd.service
journalctl -u pcloudd.service -n 50 --no-pager
pcloudc userinfo            # before login: returns Unauthenticated
pcloudc login               # interactive
pcloudc userinfo            # after login: prints account info
```

### 1.6 Config file location

| Tree | Default | Override |
|------|---------|----------|
| Config file candidate | `$HOME/.config/pcloud/config.json`, then `$HOME/.pcloud/config.json` | `--config <path>` |
| Config dir | `${XDG_CONFIG_HOME:-$HOME/.config}/pcloud/pcloud-rs/` | `PCLOUD_ROOT` |
| State dir | `${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs/` | `PCLOUD_ROOT` |
| Runtime dir | `${XDG_RUNTIME_DIR:-/run/user/$UID}/pcloud/pcloud-rs/` | `PCLOUD_ROOT` |

Validate the config:

```bash
pcloudc config validate
pcloudc config show
```

### 1.7 Inspect logs

```bash
# System service:
sudo journalctl -u pcloudd.service -f
# User service:
journalctl --user -u pcloudd.service -f
```

JSON log fields are documented in
[`reference/config.md`](../reference/config.md). Forward to a SIEM
via `systemd-journal-remote`, Filebeat, Vector, or Fluent Bit.

### 1.8 SELinux / AppArmor

The deep-dive lives in
[`platforms/linux.md` § Firewall, SELinux, AppArmor, sandboxing](./platforms/linux.md#firewall-selinux-apparmor-sandboxing).
A minimal SELinux module ships in `packaging/selinux/`:

```bash
# Build and install the SELinux policy module.
make -f /usr/share/selinux/devel/Makefile -C packaging/selinux
sudo semodule -i packaging/selinux/pcloud-rs.pp
sudo restorecon -RF /var/lib/pcloud-rs /var/log/pcloud-rs /run/pcloud-rs
```

An AppArmor profile ships at `packaging/apparmor/usr.local.bin.pcloudd`.
Treat it as a starting profile: review paths and FUSE needs for your
distribution before enforcing it. See
[`platforms/linux.md` § AppArmor](./platforms/linux.md#apparmor-debian--ubuntu).

---

## 2. macOS (Tier 2)

### 2.1 Prerequisites

- macOS 13 (Ventura) or later. Tested on 14 (Sonoma) and 15 (Sequoia).
- [`fuse-t`](https://www.fuse-t.org/) **or** legacy macFUSE.
  `fuse-t` is preferred; macFUSE requires a kernel extension and a
  Recovery-mode opt-in on Apple Silicon.
- Xcode Command Line Tools (`xcode-select --install`) for the
  signing/notarisation flow.

### 2.2 Install

No macOS release workflow publishes a signed Homebrew bottle, `.pkg`, or
notarised `.dmg` today. The Homebrew formula and package scripts are
operator-run scaffolds; build and sign them locally before evaluation.

The local package scripts drop binaries under `/usr/local/bin/` and the
user LaunchAgent template under
`~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist`.

### 2.3 Enable the launchd agent

```bash
launchctl load -w \
  ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist
launchctl list | grep com.pcloud.pcloud-rs
```

For the system-scope variant, place the plist in
`/Library/LaunchDaemons/` and `sudo launchctl bootstrap system
/Library/LaunchDaemons/com.pcloud.pcloudd.plist`.

### 2.4 Code-sign and notarise (if you build from source)

```bash
./packaging/signing/sign-macos.sh \
  --identity "Developer ID Application: Your Org (TEAMID)" \
  target/release/pcloudd target/release/pcloudc
./packaging/signing/notarize-macos.sh \
  --keychain-profile pcloud-rs-notarize \
  pcloud-rs-x.y.z.pkg
```

Gatekeeper rejects unsigned binaries on first launch from a
quarantined download. Locally built binaries may still need quarantine
attributes cleared depending on how they were transferred.

### 2.5 Troubleshooting first run

Common pitfalls:

- TCC denies "Files and Folders" access — grant via System Settings →
  Privacy & Security → Files and Folders → enable `pcloudd`.
- `fuse-t` not installed — see
  [`platforms/macos.md` § Mount setup (fuse-t)](./platforms/macos.md#mount-setup-fuse-t).
- Notarisation rejected — re-run `notarize-macos.sh` and check
  Apple's developer portal for the rejection reason.

Full per-platform deep dive:
[`platforms/macos.md`](./platforms/macos.md).

---

## 3. Windows (Tier 1 qualification target)

### 3.1 Prerequisites

- Windows 10/11 x64.
- The signed public setup bundle, which chains the checksum-pinned and
  vendor-signature-verified WinFSP 2.1 MSI.
- Administrator approval for machine-wide binary/driver installation; daemon
  runtime remains per-user.

### 3.2 Install

Verify the setup bundle's Authenticode signature and release checksum, then run
it elevated. The standalone MSI requires WinFSP to be installed already.

### 3.3 Start the per-user daemon

Run this from the interactive user's session:

```powershell
pcloudc start
pcloudc status
```

The MSI deliberately installs no SCM service. Named-pipe authentication,
DPAPI, and WinFSP mounts are bound to the user SID; a service account would
create an endpoint and vault the user cannot access.

### 3.4 Release evidence

The repository defines hosted Windows named-pipe tests, a live WinFSP
mount/read/write/unmount gate, and a strict signed MSI/Burn release job. Treat
Windows as supported only when those exact jobs pass for the release commit;
the workflow text alone is not evidence.

Per-platform deep dive: [`platforms/windows.md`](./platforms/windows.md).

---

## 4. FreeBSD (Tier 1 qualification target)

### 4.1 Prerequisites

- FreeBSD 13.2+ amd64.
- `fusefs-libs3` package (FUSE3 user-space).
- `kldload fusefs` once at boot, or via `/boot/loader.conf`:
  ```
  fusefs_load="YES"
  ```

### 4.2 Install

```sh
pkg install pcloud-rs
```

Or from source (requires Rust 1.88+):

```sh
pkg install rust pkgconf fusefs-libs3 openssl
gmake build
gmake install
```

### 4.3 rc.d enable

The shipped `rc.d` script lives at `packaging/freebsd/pcloudd.rc`:

```sh
install -m 0755 packaging/freebsd/pcloudd.rc /usr/local/etc/rc.d/pcloudd
sysrc pcloudd_enable=YES
service pcloudd start
service pcloudd status
```

OpenBSD and NetBSD scripts ship under
`packaging/openbsd/` and `packaging/netbsd/` respectively. See
[`platforms/freebsd.md`](./platforms/freebsd.md),
[`platforms/openbsd.md`](./platforms/openbsd.md), and
[`platforms/netbsd.md`](./platforms/netbsd.md).

### 4.4 Tier-3 caveat

> **Tier 3 = community best-effort.** The FreeBSD CI job runs with
> `continue-on-error: true`. Regressions on FreeBSD do not gate the
> PR pipeline. Production deployment is **not** recommended at this
> tier. Mount-cleanup on SIGTERM is also Tier-3 — operators may need
> to `umount -f` manually after an abnormal exit.

---

## 5. Hardening matrix (systemd reference)

The shipped unit
([`packaging/systemd/pcloudd.service`](https://github.com/ezechiel203/pcloud-rs/blob/main/packaging/systemd/pcloudd.service))
encodes the following hardening directives. Each is documented
inline in the unit file; the table below is for quick reference.

| Directive | Value | Why |
|-----------|-------|-----|
| `Type=` | `simple` | Daemon binds its own IPC socket and does not emit `READY=1` on the Unix serve path. |
| `WatchdogSec=` | _unset_ | Watchdog is disabled until daemon heartbeats are driven by `$WATCHDOG_USEC`; a fixed `30s` watchdog can kill a healthy idle daemon. |
| `DynamicUser=` | `yes` | Ephemeral UID; no fixed account to compromise. |
| `NotifyAccess=` | _unset_ | Not needed while `Type=simple` and `WatchdogSec=` are unset. |
| `ProtectSystem=` | `strict` | Read-only `/usr`, `/boot`, `/etc`. |
| `ProtectHome=` | `tmpfs` | `/home` invisible to the daemon. |
| `PrivateTmp=` | `yes` | Per-service `/tmp` namespace. |
| `PrivateDevices=` | `yes` | No `/dev/*` access (FUSE deployments override). |
| `PrivateUsers=` | `yes` | UID namespace; UID 0 inside is unmapped outside. |
| `ProtectKernelTunables=` | `yes` | `/proc/sys` read-only. |
| `ProtectKernelModules=` | `yes` | No `init_module(2)`. |
| `ProtectKernelLogs=` | `yes` | No `kmsg` access. |
| `ProtectControlGroups=` | `yes` | No cgroup writes. |
| `ProtectClock=` | `yes` | No clock-set syscalls. |
| `ProtectHostname=` | `yes` | No `sethostname(2)`. |
| `ProtectProc=` | `invisible` | `/proc/<other-pid>/` invisible. |
| `ProcSubset=` | `pid` | `/proc` reduced to PID entries only. |
| `LockPersonality=` | `yes` | No `personality(2)` flips (defeats certain ROP gadgets). |
| `RestrictSUIDSGID=` | `yes` | No new setuid/setgid files. |
| `RemoveIPC=` | `yes` | SysV IPC torn down on stop. |
| `UMask=` | `0077` | Owner-only on every created file. |
| `RuntimeDirectoryMode=` | `0700` | IPC socket parent dir. |
| `StateDirectoryMode=` | `0700` | SQLite + vault parent dir. |
| `LogsDirectoryMode=` | `0700` | Per-service log dir. |
| `NoNewPrivileges=` | `yes` | `prctl(PR_SET_NO_NEW_PRIVS, 1)`. |
| `CapabilityBoundingSet=` | _empty_ | All file capabilities dropped. |
| `AmbientCapabilities=` | _empty_ | No ambient caps. |
| `RestrictAddressFamilies=` | `AF_UNIX AF_INET AF_INET6` | No `AF_NETLINK`, `AF_PACKET`, etc. |
| `IPAddressDeny=` | _unset by default_ | Host firewall governs outbound traffic; `override.conf.example` can opt into `any`. |
| `IPAddressAllow=` | _unset by default_ | Only used by the optional egress allow-list drop-in. |
| `SystemCallArchitectures=` | `native` | x86_64 only on x86_64. |
| `SystemCallFilter=` | `@system-service ~@privileged @resources @obsolete @mount @debug @cpu-emulation @raw-io @reboot @swap` | Deny-by-default allow-list of system-service syscalls. |
| `SystemCallErrorNumber=` | `EPERM` | Filtered syscalls return `EPERM` not `SIGSYS`. |
| `MemoryMax=` / `MemoryHigh=` | `512M` / `384M` | Hard / soft RSS cap. |
| `CPUQuota=` | `75%` | One core max equivalent. |
| `TasksMax=` | `256` | Thread/process budget. |
| `LimitNOFILE=` | `4096` | FD budget. |
| `LimitNPROC=` | `256` | Per-user process budget. |
| `LimitCORE=` | `0` | No core dumps (vault leakage prevention). |
| `KeyringMode=` | `private` | Per-service kernel keyring. |
| `RestrictNamespaces=` | `yes` | No `unshare(2)` of new namespaces. |
| `RestrictRealtime=` | `yes` | No `SCHED_FIFO` / `SCHED_RR`. |

Operator responsibility:

- **Install `override.conf.example` only when you want systemd-level egress
  allow-listing.** Keep its `IPAddressAllow=` entries aligned with the
  pCloud API endpoints you actually use.
- **Provide auth tokens via `LoadCredentialEncrypted=`**, never via
  `Environment=`.
- **Install the FUSE drop-in** only on hosts that mount.

---

## 6. Log rotation

### 6.1 systemd-journald

The default. Rotation is automatic; configure caps in
`/etc/systemd/journald.conf`:

```ini
[Journal]
SystemMaxUse=2G
RuntimeMaxUse=512M
MaxFileSec=1week
MaxRetentionSec=4week
```

### 6.2 File output (non-systemd or supplementary)

If the daemon is configured to log to a file (`logging.path` in
`config.json`), use `logrotate`. Drop this in
`/etc/logrotate.d/pcloud-rs`:

```
/var/log/pcloud-rs/*.log {
    daily
    rotate 14
    compress
    delaycompress
    missingok
    notifempty
    create 0600 pcloud-rs pcloud-rs
    sharedscripts
    postrotate
        systemctl --signal=SIGUSR1 kill pcloudd.service 2>/dev/null || true
    endscript
}
```

The daemon re-opens its log file on `SIGUSR1`. Without the postrotate
hook, `logrotate` will rotate the inode but the daemon will keep
writing to the old file descriptor.

---

## 7. Backup and restore

### 7.1 What to back up

| Path | Restorable? | Notes |
|------|-------------|-------|
| `<state>/store.sqlite` | Yes | SQLite store; contains queue, sync-root list, journal pointers. |
| `<state>/journal/` | Yes | Writeback intents; replay-safe across hosts (same UID). |
| `<state>/audit.log` | Yes | Append-only; restoring breaks the chain at the seam — rotate to a fresh log on restore. |
| `<data>/auth-vault.json` | Per-host only | Wrapped tokens are bound to the daemon UID. Backing up across hosts is unsafe; back up only for same-host disaster recovery. |
| `<data>/crypto-profile.json` | Yes | Wrapped master key; safe across hosts (decrypt requires the crypto password). |
| `<config>/config.json` | Yes | Plain text; safe. |
| Mount staging dir | No | Ephemeral; let the daemon recreate. |
| Mount-orphan registry (Linux) | No | Reaped on daemon start. |

### 7.2 Restore procedure

```bash
systemctl --user stop pcloudd.service
# Restore files preserving owner/mode (rsync -aHAX or tar -p).
rsync -aHAX backup/pcloud-rs/ "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs/"
chmod 700 "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs"
chmod 600 "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs/store.sqlite"
systemctl --user start pcloudd.service
journalctl --user -u pcloudd.service -n 100 --no-pager
```

If the daemon refuses to open the vault after restore (mode/owner
drift), repair as documented in
[Troubleshooting § 7](./troubleshooting.md#7-permission-errors-on-socket--vault--mount).

---

## 8. Upgrade

### 8.1 In-place restart

The daemon performs SQLite schema migrations on startup. The migration
framework is forward-only; downgrading after a migration ran requires
restoring from backup. Always take a `store.sqlite` backup before
upgrading.

```bash
# Take backup.
cp "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs/store.sqlite" \
   "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs/store.sqlite.backup-$(date +%Y%m%d)"

# Stop, install new binaries, start.
systemctl --user stop pcloudd.service
sudo install -Dm0755 new/pcloudd /usr/bin/pcloudd
sudo install -Dm0755 new/pcloudc /usr/bin/pcloudc
systemctl --user start pcloudd.service

# Confirm migration ran cleanly.
journalctl --user -u pcloudd.service -n 50 --no-pager | grep -i 'migration\|schema'
sqlite3 -readonly "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud/pcloud-rs/store.sqlite" \
  'PRAGMA user_version;'
```

### 8.2 What survives a restart

- SQLite store (with migrations applied).
- Journal (replayed on next sync sweep).
- Vault (token re-validated against the API on first call).
- Crypto profile (re-unlocked on next `crypto unlock`).
- Sync-root list (reattached at startup).

### 8.3 What requires user action after upgrade

- An expired auth token re-prompts for login. Recovery codes still
  work.
- A mount that was active at upgrade time is **not** automatically
  re-mounted. Re-issue `pcloudc mount`.
- Crypto must be re-unlocked after every daemon restart by design
  (ADR 0007 — passwords are not persisted).

### 8.4 Vault-format version check

The daemon refuses to open a vault whose format version does not
match the binary's expected version. The error message points at the
required upgrade path. Roll forward, never roll back.

---

## 9. Cross-references

- Fleet rollout topology: [`deployment.md`](./deployment.md)
- Per-platform deep dives: [`platforms/`](./platforms/linux.md)
- Runbook (ops procedures): [`runbook.md`](./runbook.md)
- Upgrade detail: [`upgrade.md`](./upgrade.md)
- Troubleshooting: [`troubleshooting.md`](./troubleshooting.md)
- Security operations: [`security-operations.md`](./security-operations.md)
- Packaging matrix: [`packaging-matrix.md`](./packaging-matrix.md)
- Reference packaging: [`reference/packaging.md`](../reference/packaging.md)

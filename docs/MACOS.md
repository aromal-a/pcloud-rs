# macOS Platform Guide

This document covers the macOS-specific behavior of `pcloud-rs`. It is based
on the code as it exists today. Where behavior is scaffolded but not yet
live-tested on a real Mac, that is stated explicitly. Do not treat untested
sections as production guidance.

**Current macOS status (2026-04-17):** We are now running on a real macOS
host. FUSE callbacks are wired and bring-up is in progress under
`bd-1du.4.6`. Auth, sync, transfers, and the CLI work on macOS; the
mounted-drive end-to-end integration tests are the remaining work.

---

## Requirements

### macOS version

| macOS            | Arch              | Status                              |
|------------------|-------------------|-------------------------------------|
| 12 (Monterey)    | x86_64            | Build-only; no mount verification   |
| 13 (Ventura)     | x86_64, arm64     | Build-only; no mount verification   |
| 14 (Sonoma)      | arm64 (primary)   | In active use — build + bring-up in progress |
| 15 (Sequoia)     | arm64             | Expected to build; untried          |
| 11 (Big Sur) and older | any        | Not supported — fuse-t requires 12+ |

Apple Silicon (arm64) is the primary target going forward. Intel (x86_64)
builds are available but will fall out of routine CI before end-of-fork.

### FUSE backend

The pCloud FUSE mount requires a userspace FUSE library. Two backends are
supported:

- **fuse-t** (recommended): <https://www.fuse-t.org/>. No kernel extension
  required; bridges FUSE over an NFS loopback. Install once; no reboot.
- **macFUSE** (fallback): <https://macfuse.github.io/>. Requires a kernel
  extension that must be approved in System Settings → Privacy & Security.

The daemon defaults to fuse-t. To override:

```bash
export PCLOUD_MACOS_FUSE_BACKEND=macfuse   # or: fuse-t (default), auto
```

`auto` probes fuse-t first, then macFUSE. Mixing both on the same host is
not supported.

The fuse-t library is discovered at runtime via `dlopen` from the following
candidate paths (first existing path wins):

- `/usr/local/lib/libfuse-t.dylib`
- `/opt/homebrew/lib/libfuse-t.dylib`
- `/Library/Application Support/fuse-t/lib/libfuse-t.dylib`

For macFUSE:

- `/usr/local/lib/libfuse.dylib`
- `/opt/homebrew/lib/libfuse.dylib`

The binary does not link against either library at build time. If neither
library is found at mount time, `MountError::Unsupported` is returned with
a human-readable install hint.

### Rust toolchain

```bash
rustup toolchain install stable
rustup target add x86_64-apple-darwin      # Intel (if needed)
rustup target add aarch64-apple-darwin     # Apple Silicon (if needed)
```

Minimum tested Rust version: whatever the workspace `rust-version` field
specifies (check `Cargo.toml`). The workspace uses the 2021 edition.

---

## Installation

### From Homebrew (recommended)

```bash
brew install rust fuse-t          # runtime dependencies
brew tap pcloud-rs/pcloud-rs
brew install pcloud-rs
```

The formula installs `pcloudc` (CLI) and `pcloudd` (daemon) and registers a
`brew services` launchd unit backed by the template in
`packaging/homebrew/pcloud-rs.rb`.

### From a signed .pkg

Releases ship a notarized, signed `.pkg` for MDM deployment (Jamf, Kandji,
Mosyle):

```bash
# Verify signature before installing
pkgutil --check-signature pcloud-rs-<version>.pkg
spctl --assess --type install pcloud-rs-<version>.pkg

# Install
sudo installer -pkg pcloud-rs-<version>.pkg -target /
```

The `.pkg` drops a launchd plist template into
`/Library/LaunchAgents/com.pcloud.pcloud-rs.plist`.

### From source

```bash
# 1. Install build-time and runtime dependencies
brew install rust fuse-t pkg-config

# 2. Clone and build
git clone https://github.com/pcloudcom/pcloud-rs
cd pcloud-rs
cargo build --release -p pcloud-daemon -p pcloud-cli
```

Build times on an M2 Pro (16 GiB RAM): clean release ~3–4 minutes;
incremental recompile after a single-crate change ~10–30 seconds.

### Signing and notarization (release pipeline)

Releases are signed with a Developer ID Application certificate and
notarized by Apple before distribution.

```bash
# Codesign
bash packaging/signing/sign-macos.sh \
  --identity "Developer ID Application: <org> (<team id>)" \
  --entitlements packaging/macos/entitlements.plist \
  target/release/pcloud-daemon \
  target/release/pcloud-cli

# Notarize the .pkg
bash packaging/signing/notarize-macos.sh \
  --bundle-id com.pcloud.pcloudd \
  --team-id <team id> \
  --apple-id <apple id> \
  pcloud-rs-<version>.pkg
```

The entitlements template at `packaging/macos/entitlements.plist` is
deliberately minimal. The only entitlements enabled are outbound network
access (`com.apple.security.network.client`) and
`com.apple.security.cs.disable-library-validation`, which is required to load
fuse-t's dylib (signed by a different Team ID) under the hardened runtime.
The `disable-library-validation` entitlement was enabled as part of
`bd-1du.4.6` bring-up.

For locally-built binaries that trigger Gatekeeper quarantine:

```bash
xattr -dr com.apple.quarantine target/release/pcloud-daemon
xattr -dr com.apple.quarantine target/release/pcloud-cli
```

---

## Key Differences from Linux

### FUSE implementation: fuse-t / macFUSE vs Linux fuser crate

On Linux, `pcloud-fs` uses the `fuser` Rust crate, which talks to the
kernel's native FUSE subsystem via `/dev/fuse`.

On macOS, the `fuser` crate is not used. Instead, the code
(`crates/pcloud-fs/src/platform/macos.rs` and `macos_ffi.rs`) uses a
hand-rolled FFI binding against the libfuse 2.9 low-level API exported by
fuse-t or macFUSE. The libfuse dylib is loaded at runtime via `dlopen` with
`RTLD_GLOBAL` so the flat-namespace `extern "C"` symbols resolve.

The fuse-t backend bridges FUSE to an NFS loopback server rather than
talking directly to a kernel VFS module. This has observable consequences:

- The `allow_other` mount option is passed but NFS access routing means
  cross-user access policy is handled differently than on Linux.
- A `defer_permissions` option is added on macOS so the FUSE mode/uid/gid
  bits govern access instead of the NFS client's cached permissions.
- Write operations require `-o rw` to be in the fuse-t argv, otherwise the
  NFS server exports read-only and kernel-level creates/writes fail before
  any FUSE thunk fires.
- fuse-t may not carry the `fh` (file handle) field across NFS request
  boundaries; the `read` thunk handles an `fh == 0` case with on-demand
  re-open to work around this.

The macOS FUSE module is **entirely disabled on non-macOS targets** via
`#[cfg(target_os = "macos")]` at the module level. The Linux workspace
builds and tests are unaffected by any macOS-only code.

**Bring-up status:** The full read+write FUSE callback set is wired (init,
destroy, lookup, getattr, open, read, readdir, release, write, create,
unlink, mkdir, rmdir, rename, flush, fsync, setattr, statfs). The session
loop runs on a dedicated background thread. Bring-up is now in progress on
a real macOS host; end-to-end integration tests are tracked under `bd-1du.4.6`.

### Keychain: macOS Keychain vs Linux file vault

On Linux, auth token persistence uses a file-backed vault (`0600` file,
`0700` parent directory) via the `FileVault` backend in
`crates/pcloud-daemon/src/auth_vault.rs`.

On macOS, auth token persistence uses the system Keychain via the
`KeychainVault` backend in
`crates/pcloud-daemon/src/vault/keychain.rs`, which calls the
`security-framework` crate. Tokens are stored as generic password items
under service identifier `com.pcloud.pcloud-rs` and account identifier
`pcloud-auth-token`, scoped to the current user's login keychain. The bytes
are wrapped in `SecretString` on read so zeroize-on-drop applies.

Token persistence remains **explicit opt-in** on both platforms. Passwords
are never persisted on either platform.

Error mapping: `security-framework` `SecError` values are surfaced as
`AuthVaultError::Io` with the `OSStatus` code embedded in the message for
post-mortem debugging.

### Config paths: ~/Library/... vs XDG directories

On Linux/BSD the daemon follows the XDG Base Directory Specification. On
macOS it follows Apple's standard directory layout via the `directories`
crate (`ProjectDirs::from("com", "pcloud", "pcloud-rs")`):

| Role        | macOS path                                                      | Linux fallback path                            |
|-------------|-----------------------------------------------------------------|------------------------------------------------|
| Config      | `~/Library/Application Support/com.pcloud.pcloud-rs`           | `$XDG_CONFIG_HOME/pcloud/pcloud-rs`            |
| State/data  | `~/Library/Application Support/com.pcloud.pcloud-rs`           | `$XDG_DATA_HOME/pcloud/pcloud-rs`              |
| Cache       | `~/Library/Caches/com.pcloud.pcloud-rs`                        | `$XDG_CACHE_HOME/pcloud/pcloud-rs`             |
| Runtime     | `~/Library/Caches/com.pcloud.pcloud-rs/pcloud-rs-runtime`      | `$XDG_RUNTIME_DIR/pcloud/pcloud-rs`            |
| IPC socket  | `<runtime>/pcloud.sock`                                         | `<runtime>/pcloud.sock`                        |
| Auth vault  | `<config>/auth_token`                                           | `<config>/auth_token`                          |

Note: macOS has no equivalent of `$XDG_RUNTIME_DIR` (a true per-boot
tmpfs). The runtime directory falls back to a subdirectory of the cache
dir, which is long-lived, not a tmpfs. All IPC socket and PID file paths
are therefore under `~/Library/Caches/...` on macOS.

If `XDG_*` environment variables are set on macOS, they take precedence.
Do not mix both unless you understand the override order.

The `PCLOUD_ROOT` environment variable overrides all four directories to
subdirectories of the given root, useful for multi-instance isolation and
testing.

Create the required directories once:

```bash
install -d -m 0700 \
  "$HOME/Library/Application Support/com.pcloud.pcloud-rs" \
  "$HOME/Library/Caches/com.pcloud.pcloud-rs" \
  "$HOME/Library/Caches/com.pcloud.pcloud-rs/pcloud-rs-runtime" \
  "$HOME/Library/Logs/pcloud-rs"
```

### IPC socket location

On Linux the IPC socket typically lives under `$XDG_RUNTIME_DIR`
(e.g. `/run/user/1000/pcloud/pcloud-rs/pcloud.sock`). That directory is
a kernel-managed tmpfs with a 1 GiB default quota.

On macOS the socket lives under the runtime subdir of the cache:
`~/Library/Caches/com.pcloud.pcloud-rs/pcloud-rs-runtime/pcloud.sock`.

When running as a user LaunchAgent, `$TMPDIR` resolves to a Darwin
per-user sandbox temp dir (e.g. `/var/folders/...`), which means other
local users cannot reach the socket without explicit filesystem access even
if the path were world-readable (it is not: mode is `0600`).

### Peer credential check: getpeereid vs SO_PEERCRED

On Linux the daemon uses `SO_PEERCRED` getsockopt to read the connecting
peer's UID. On macOS (and other BSDs), `SO_PEERCRED` is not available; the
daemon uses `getpeereid(3)` instead. Both paths enforce the same security
policy: a connecting client whose effective UID does not match the daemon's
UID is rejected with `Unauthorized`.

### Signal handling: no SIGRTMIN on macOS

The daemon uses `SIGTERM` and `SIGINT` for graceful shutdown on both
platforms. `SIGRTMIN`-based real-time signals (used by some Linux process
supervisors) do not exist on macOS; the daemon does not rely on them.
macOS-specific signal behavior: `SIGHUP` is handled for log rotation if
wired; `SIGUSR1` / `SIGUSR2` are reserved for future use.

### Mount discovery: getmntinfo vs /proc/mounts

On Linux the daemon reads `/proc/self/mountinfo` to discover orphaned FUSE
mounts on startup.

On macOS, `/proc` does not exist. The `MacosMountinfoReader` struct
(in `crates/pcloud-fs/src/platform/macos.rs`) calls `getmntinfo(3)` to
enumerate the kernel mount table. pcloud-rs mounts carry the private
`fsname=pcloud-rs` source identity. The reader requires both a FUSE-family
filesystem type and that pCloud source identity, then emits a
`/proc/self/mountinfo`-shaped payload. It therefore cannot claim an unrelated
sshfs, rclone, or generic macFUSE volume during orphan recovery.

### Daemon management: launchd vs systemd

On Linux the daemon is typically managed by systemd. On macOS it is managed
by launchd. The repository ships two plist templates:

| File | Scope | Install location |
|------|-------|-----------------|
| `packaging/macos/com.pcloud.pcloud-rs.plist` | User LaunchAgent | `~/Library/LaunchAgents/` |
| `packaging/macos/com.pcloud.pcloudd.plist` | System LaunchDaemon | `/Library/LaunchDaemons/` |

The LaunchAgent is the recommended path for personal use. The LaunchDaemon
is for fleet-managed deployments where the daemon runs as a dedicated
service account (`_pcloudd`).

### Legacy path migration

The C client used `~/.pcloud/` on Linux. That path is consulted as a
read-only migration source **on Linux only** when
`PCLOUD_MIGRATE_LEGACY_PATHS=1` is set. On macOS `legacy_linux_home()`
returns `None` unconditionally; no `~/.pcloud/` migration path exists on
macOS.

---

## macOS-specific Configuration

### Daemon configuration

The daemon reads its config from the `PCLOUD_ROOT`-derived or Apple
standard directory. Minimal production config:

```toml
[profile]
environment = "production"

[paths]
# Defaults are derived automatically; override only if needed.
# config_dir  = "/Users/alice/Library/Application Support/com.pcloud.pcloud-rs"
# state_dir   = "/Users/alice/Library/Application Support/com.pcloud.pcloud-rs"
# cache_dir   = "/Users/alice/Library/Caches/com.pcloud.pcloud-rs"
# runtime_dir = "/Users/alice/Library/Caches/com.pcloud.pcloud-rs/pcloud-rs-runtime"

[mount]
enabled = true
path    = "/Users/alice/pCloudDrive"
policy  = "default"
```

### FUSE backend selection environment variable

| Variable | Values | Default |
|----------|--------|---------|
| `PCLOUD_MACOS_FUSE_BACKEND` | `fuse-t`, `macfuse`, `auto` | `fuse-t` |

### Environment variables read by the daemon

The following variables are recognized and honored (cross-checked against
`crates/pcloud-config/src/env.rs`):

| Variable | Effect |
|----------|--------|
| `PCLOUD_ROOT` | Override all four managed directories to subdirs of this root |
| `PCLOUD_ENV` | `production` or `development` |
| `PCLOUD_API_HOST` | pCloud API hostname override |
| `PCLOUD_API_SERVER_NAME` | TLS SNI override |
| `PCLOUD_LOG_LEVEL` | `trace`, `debug`, `info`, `warn`, `error` |
| `PCLOUD_DURABLE_AUTH_TOKENS` | `1` to opt in to persistent token storage |
| `PCLOUD_MACOS_FUSE_BACKEND` | `fuse-t` (default), `macfuse`, `auto` |

The following variables appear in the launchd plist templates but are **not
read** by the Rust daemon; they are present for operator readability only:
`PCLOUD_HOME`, `PCLOUD_CONFIG` (CLI only, not daemon), `PCLOUD_AUTH_VAULT`,
`PCLOUD_API_SERVER`, `PCLOUD_IPC_SOCKET`, `PCLOUD_MOUNT_POINT`.

### launchd User LaunchAgent (auto-start on login)

The plist template in `packaging/macos/com.pcloud.pcloud-rs.plist`
requires one substitution before installation: replace every
`{{USER_HOME}}` placeholder with an absolute path (launchd does not expand
`$HOME` in plist values).

```bash
# 1. Substitute $HOME
sed "s|{{USER_HOME}}|$HOME|g" \
    packaging/macos/com.pcloud.pcloud-rs.plist \
    > ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist

# 2. Create log directory
mkdir -p ~/Library/Logs/pcloud-rs

# 3. Load and start
launchctl bootstrap gui/$(id -u) \
    ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist
launchctl enable gui/$(id -u)/com.pcloud.pcloud-rs
launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloud-rs
```

Inspect:

```bash
launchctl print gui/$(id -u)/com.pcloud.pcloud-rs
tail -f ~/Library/Logs/pcloud-rs/pcloud-rs.err.log
```

Stop:

```bash
launchctl bootout gui/$(id -u)/com.pcloud.pcloud-rs
```

### launchd System LaunchDaemon (auto-start at boot, fleet deployments)

The system daemon runs as the dedicated service account `_pcloudd`. Create
the account once:

```bash
sudo dscl . -create /Users/_pcloudd UniqueID 299
sudo dscl . -create /Users/_pcloudd PrimaryGroupID 299
sudo dscl . -create /Users/_pcloudd UserShell /usr/bin/false
sudo dscl . -create /Users/_pcloudd NFSHomeDirectory /var/lib/pcloudd
```

Install and start:

```bash
# Install the plist
sudo install -m 0644 -o root -g wheel \
    packaging/macos/com.pcloud.pcloudd.plist \
    /Library/LaunchDaemons/com.pcloud.pcloudd.plist

# Prepare directories
sudo install -d -o _pcloudd -g _pcloudd -m 0700 /var/lib/pcloudd
sudo install -d -o _pcloudd -g _pcloudd -m 0750 /var/log/pcloudd

# Load and start
sudo launchctl load -w /Library/LaunchDaemons/com.pcloud.pcloudd.plist
```

Inspect:

```bash
sudo launchctl list | grep com.pcloud.pcloudd
sudo tail -f /var/log/pcloudd/pcloudd.err.log
```

### launchd cheat-sheet

| Action | Command |
|--------|---------|
| Load (user) | `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist` |
| Enable | `launchctl enable gui/$(id -u)/com.pcloud.pcloud-rs` |
| Start/restart | `launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloud-rs` |
| Stop | `launchctl bootout gui/$(id -u)/com.pcloud.pcloud-rs` |
| Status | `launchctl print gui/$(id -u)/com.pcloud.pcloud-rs` |
| Tail stdout | `tail -f ~/Library/Logs/pcloud-rs/pcloud-rs.out.log` |
| Tail stderr | `tail -f ~/Library/Logs/pcloud-rs/pcloud-rs.err.log` |
| Unified log | `log stream --predicate 'process == "pcloudd"' --info` |

### Auto-mount on Login

To have the daemon mount the pCloud drive automatically when it starts (and
therefore at each login when running as a LaunchAgent), set the
`PCLOUD_AUTO_MOUNT_PATH` environment variable in your LaunchAgent plist:

```xml
<key>EnvironmentVariables</key>
<dict>
    <key>PCLOUD_AUTO_MOUNT_PATH</key>
    <string>/Users/alice/pCloudDrive</string>
</dict>
```

When `PCLOUD_AUTO_MOUNT_PATH` is set, the daemon will attempt to mount the
pCloud drive at the given path during startup, immediately after the IPC
server is bound and authentication is restored from the vault. The mount path
must be an empty directory and must not be inside a SIP-protected location.

If the mount fails at startup (e.g. fuse-t not installed, authentication not
yet configured), the daemon still starts and serves IPC requests normally.
The mount can be retried manually with `pcloudc mount <path>`.

To create the mountpoint directory:

```bash
mkdir -p ~/pCloudDrive
```

Combine with `PCLOUD_DURABLE_AUTH_TOKENS=1` and a populated Keychain token
to get a fully automatic login-time mount:

```xml
<key>EnvironmentVariables</key>
<dict>
    <key>PCLOUD_DURABLE_AUTH_TOKENS</key>
    <string>1</string>
    <key>PCLOUD_AUTO_MOUNT_PATH</key>
    <string>/Users/alice/pCloudDrive</string>
</dict>
```

---

## Security Notes

### macOS Keychain integration for token storage

The `KeychainVault` backend (`crates/pcloud-daemon/src/vault/keychain.rs`)
stores auth tokens as generic password items in the user's login Keychain
under service `com.pcloud.pcloud-rs`. This is the macOS equivalent of the
`0600` file vault on Linux.

Key properties:
- Secrets never touch disk in plaintext; the system Keychain enforces
  per-user ACLs.
- On read, bytes are wrapped in `SecretString` and will be zeroized on drop.
- Token persistence is still **explicit opt-in** (`PCLOUD_DURABLE_AUTH_TOKENS=1`).
- Passwords are **never** persisted, on any platform.
- `errSecItemNotFound` (-25300) is treated as a clean empty-vault condition,
  not an error.

A LaunchAgent (not a LaunchDaemon) must be used for user-context token
storage because system LaunchDaemons cannot access the per-user login
Keychain.

### Gatekeeper considerations

Any unsigned or un-notarized binary will be quarantined by Gatekeeper on
macOS 12+. The release pipeline signs and notarizes; local builds can be
unblocked with:

```bash
xattr -dr com.apple.quarantine target/release/pcloudd
xattr -dr com.apple.quarantine target/release/pcloudc
```

Do not distribute unsigned or un-notarized binaries to end users.

### Hardened runtime and entitlements

The entitlements file at `packaging/macos/entitlements.plist` is minimal by
design:

- `com.apple.security.network.client = true`: required for HTTPS to the
  pCloud API.
- `com.apple.security.cs.disable-library-validation = true`: **now enabled**
  to allow fuse-t's dylib (signed by a different Team ID) to load under the
  hardened runtime. This is the most security-sensitive entitlement in the
  set; it was reviewed and enabled as part of `bd-1du.4.6` bring-up.
- JIT, executable memory, and dyld env variable entitlements are
  explicitly set to `false` to prevent future accidental enablement.

### SIP (System Integrity Protection) and mount restrictions

SIP restricts kernel extension loading on macOS 11+. fuse-t avoids this
entirely by bridging FUSE over NFS loopback with no kernel extension.

macFUSE requires a kernel extension that must be approved in
System Settings → Privacy & Security. SIP does not prevent this approval,
but it does prevent the extension from loading silently; user interaction
or MDM pre-approval is always required.

SIP does protect certain filesystem paths from being used as mount points
(e.g. paths under `/System`). The daemon enforces its own mountpoint
validation (`validate_mountpoint`) before calling into fuse-t, but SIP
restrictions on OS-protected paths are enforced by the kernel and will
surface as `fuse_mount` returning NULL.

### TCC (Transparency, Consent, and Control)

The daemon does not need Full Disk Access (FDA) for its own operation. It
accesses:
- Its own `~/Library/Application Support/...` and `~/Library/Caches/...`
  directories (no TCC required).
- User-specified sync roots (TCC grant required for paths outside
  `~/Documents`, `~/Desktop`, `~/Downloads`, etc. in restricted zones).

If users see "Operation not permitted" errors on sync-root paths, the
correct fix is to grant the daemon access in
System Settings → Privacy & Security → Files and Folders, not to enable FDA.

For MDM-managed fleets, pre-approve the sync-root TCC grant and the fuse-t
system extension via a configuration profile to avoid interactive prompts.

---

## Troubleshooting

### fuse-t not found

Symptom: `MountError::Unsupported` with message containing "fuse-t not
installed".

Cause: neither of the candidate dylib paths exists.

Fix:

```bash
brew install --cask fuse-t
# Verify installation
pkgutil --pkg-info=io.fuse-t.pkg.core
ls /usr/local/lib/libfuse-t.dylib || ls /opt/homebrew/lib/libfuse-t.dylib
```

If using macFUSE instead:

```bash
export PCLOUD_MACOS_FUSE_BACKEND=macfuse
# Verify macFUSE is installed and kext approved in System Settings
```

### Mount permission denied / EPERM

Common causes:

1. **TCC missing**: the daemon does not have Files and Folders access for
   the target sync-root directory. Grant in System Settings → Privacy &
   Security → Files and Folders.

2. **SIP-protected path**: the mountpoint path is under a SIP-protected
   directory. Use a path under the user's home or `/Volumes/`.

3. **fuse-t system extension not approved**: first-time fuse-t usage
   triggers a TCC prompt. Approve in System Settings, or pre-approve via
   MDM.

4. **macFUSE kext not approved**: if using macFUSE, the kernel extension
   must be approved in System Settings → Privacy & Security after install.

5. **fuse-t NFS bridge requires `-o rw`**: without this option the NFS
   server exports read-only and writes fail before any FUSE thunk fires.
   This is set automatically by `build_fuse_args`; if bypassing the normal
   mount path, ensure `-o rw` is in the fuse_args argv.

Wedged mount recovery:

```bash
diskutil unmount force ~/pCloudDrive
# or:
sudo umount -f ~/pCloudDrive
# then restart the daemon
launchctl kickstart -k gui/$(id -u)/com.pcloud.pcloud-rs
```

### Keychain access denied

Symptom: `AuthVaultError::Io` with message containing `"keychain error
(OSStatus -25243)"` (or similar negative OSStatus code).

Cause: the process does not have Keychain access. This typically happens
when:
- The daemon is running as a LaunchDaemon (system service) and trying to
  access the per-user login Keychain. System daemons cannot access user
  Keychains. Use a LaunchAgent instead.
- The Keychain is locked. This can happen after a fast-user switch or when
  Screen Saver security is enabled. The daemon will surface this as an
  error on the next token load; re-authenticate to repopulate the vault.
- The first time the app accesses the Keychain, macOS shows an authorization
  dialog. If running headlessly (e.g. SSH session), unlock the Keychain
  manually: `security unlock-keychain ~/Library/Keychains/login.keychain-db`.

### Common macOS-specific errors

| Error | Cause | Fix |
|-------|-------|-----|
| `fuse_mount returned NULL` | fuse-t kext/extension not loaded or mountpoint rejected | Verify fuse-t installation; check mountpoint is a valid empty directory |
| `fuse_lowlevel_new returned NULL` | fuse-t ABI mismatch — wrong dylib version | Reinstall fuse-t; check `PCLOUD_MACOS_FUSE_BACKEND` |
| `failed to load fuse-t backend: dlopen returned NULL` | Dylib not at expected path or signature validation failure | Reinstall fuse-t; verify `com.apple.security.cs.disable-library-validation` entitlement |
| `launchctl bootstrap ... Load failed: 5: Input/output error` | Malformed plist | `plutil -lint ~/Library/LaunchAgents/com.pcloud.pcloud-rs.plist` |
| `socket missing` from `pcloudc status` | CLI running under different user session | Do not use `sudo pcloudc`; run as the same user as the daemon |
| `EACCES` on `~/Library/Application Support/com.pcloud.pcloud-rs` | Wrong owner after Time Machine restore | `sudo chown -R $(id -u):$(id -g) ~/Library/Application\ Support/com.pcloud.pcloud-rs && chmod -R go-rwx ~/Library/Application\ Support/com.pcloud.pcloud-rs` |
| Clock drift → 401 from API | System clock skew | `sudo systemsetup -setusingnetworktime on` |

---

## Known Limitations vs Linux

### Mounted drive / FUSE

FUSE is wired and actively being tested on real macOS hardware. The
`disable-library-validation` entitlement is now enabled so the fuse-t dylib
loads under the hardened runtime. End-to-end integration tests (mount/unmount,
readdir, read, and write) are the remaining work. Tracked under `bd-1du.4` /
`bd-1du.4.6`. Until integration tests pass, treat the mount feature as
pre-production on macOS.

### No XDG runtime directory (tmpfs)

macOS has no equivalent of Linux's `$XDG_RUNTIME_DIR` tmpfs. The runtime
directory falls back to `~/Library/Caches/com.pcloud.pcloud-rs/pcloud-rs-runtime`,
which is persistent across reboots and not a memory-backed filesystem.
IPC socket files left over from a crash are stale on disk (not automatically
cleaned up on boot the way they would be on a tmpfs). The daemon now performs
a startup sweep: on macOS, any `*.sock` files in the runtime directory are
removed before the IPC listener is bound, so a clean slate is guaranteed
after a system reboot or daemon crash.

### No systemd watchdog / socket activation

The daemon does not implement systemd socket activation or `sd_notify`.
On macOS, launchd socket activation is now implemented via
`launch_activate_socket`. To enable it, uncomment the `Sockets` block in the
LaunchAgent plist (`packaging/macos/com.pcloud.pcloud-rs.plist`); the daemon
will receive the pre-bound socket descriptor from launchd instead of binding
one itself. This is the recommended path for production LaunchAgent
deployments.

### APFS behavioral differences

- APFS uses nanosecond timestamps. The FUSE stat layer now populates
  `st_mtime_nsec`, `st_ctime_nsec`, and `st_birthtime` fields in addition to
  the second-granularity `st_mtime`, `st_ctime`, and `st_atime` values. Full
  sub-second timestamp preservation through the FUSE layer requires
  live verification under `bd-1du.4.6`.
- APFS supports copy-on-write clones (`clonefile`). The FUSE adapter does
  not expose a `copy_file_range` or clone operation; clone operations
  initiated on files inside the mount will fall back to byte-copy.
- APFS snapshot paths (read-only) are rejected as sync roots at
  registration time. This is intentional.

### Secure Enclave

The Apple Silicon Secure Enclave is not used. Auth tokens are stored in the
system Keychain (which may or may not use the Secure Enclave depending on
Keychain item class and hardware). Crypto master keys are held in
`SecretBytes` in process memory and never persisted in any form.

### macOS-only limitations not present on Linux

- No `SIGRTMIN`-based signals. Some Linux process management tooling relies
  on real-time signals; these cannot be used on macOS.
- fuse-t NFS bridge can reset the `fh` (file handle) field across NFS
  request sequences (observed on some NFSv4 client flows). The `read`
  thunk handles this with an on-demand re-open fallback, but this adds
  latency that Linux FUSE does not incur.
- `chmod`/`chown`/`utimens` mutations via `setattr` are currently accepted
  as no-ops (the reply returns a refreshed attribute snapshot but does not
  forward the change to the remote). Full `setattr` coverage lands with
  `bd-1du.4.6`.
- `statfs` reports live pCloud account quota through the canonical backend.
  If the quota RPC fails, the filesystem call fails with `EIO`; local staging
  capacity and synthetic cloud capacity are never substituted.

### Performance

No direct benchmark comparisons have been done between the macOS fuse-t
path and the Linux FUSE path. The NFS loopback layer in fuse-t adds at
least one extra round-trip per operation compared to direct kernel-FUSE
communication. For workloads with many small files this overhead is expected
to be measurable; for large sequential transfers it is expected to be
negligible.

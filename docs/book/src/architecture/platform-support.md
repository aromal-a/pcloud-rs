# Platform Support

This page is the **per-platform capability matrix** for the five core
abstractions the Rust rewrite exposes: mount layer (FUSE), local IPC,
peer authentication, vault / secret storage, user-facing integration
(clipboard, notifications). For the tiered policy (T1 / T2 / T3) and
engineering effort estimates see
[`PLAN_CROSSPLATFORM.md`](../../../PLAN_CROSSPLATFORM.md). For
the packaging-side view see
[Operations → Packaging Matrix](../operations/packaging-matrix.md).

> **Honesty note (2026-04-16).** Only the **Linux** mount path is
> currently live-tested on hardware. macOS fuse-t, Windows WinFSP, and
> *BSD `fusefs` mounts are scaffolded (abstraction + binding layer) but
> have not been verified end-to-end on target hardware. Packaging and
> IPC paths are independent of the mount runtime and are further along.

## Capability matrix

| Capability              | Linux (T1)                    | macOS 12+ (T1)                          | Windows 10/11 (T1)                           | FreeBSD 13+ (T2)              | OpenBSD 7.x (T3)              | NetBSD 10 (T3)                |
|-------------------------|-------------------------------|-----------------------------------------|----------------------------------------------|-------------------------------|-------------------------------|-------------------------------|
| **Mount backend**       | `fuser` → libfuse3            | **fuse-t** via direct `libfuse-t.dylib` FFI (scaffolded) | **WinFSP 2.x** via `winfsp` crate (scaffolded) | `fuser` → fusefs (scaffolded) | `fuser` → fusefs (scaffolded) | `fuser` → refuse (scaffolded) |
| **Mount live-verified** | **yes**                       | no (hardware-bound)                     | no (hardware-bound)                          | no                            | no                            | no                            |
| **Local IPC transport** | `AF_UNIX` (abstract → fallback to filesystem) | `AF_UNIX` under `~/Library/Application Support/pcloud-rs/` | **Named pipe** (`\\.\pipe\pcloud-rs`), AF_UNIX fallback on Win10 1803+ | `AF_UNIX`                    | `AF_UNIX`                     | `AF_UNIX`                     |
| **Peer-cred check**     | `SO_PEERCRED` (uid/gid/pid)   | `LOCAL_PEERCRED` + `getpeereid(3)`      | `GetNamedPipeClientProcessId` + `OpenProcessToken` → SID match | `getpeereid(3)`              | `getpeereid(3)`              | `getpeereid(3)`              |
| **Socket ACL**          | `0600`, owner-only dir `0700` | `0600`, owner-only dir                  | Named-pipe SD: owner + `SYSTEM` only         | `0600`, owner-only dir        | `0600`, owner-only dir        | `0600`, owner-only dir        |
| **Vault backend**       | **libsecret** (Secret Service) → file fallback (`0600`) | **Keychain Services** (`SecItem*`)      | **DPAPI** (`CryptProtectData`, user scope)   | file fallback (`0600`)        | file fallback (`0600`)        | file fallback (`0600`)        |
| **Keyring crate path**  | `pcloud-secret` + `secret-service` | `pcloud-secret` + `security-framework` | `pcloud-secret` + `windows-sys` DPAPI        | `pcloud-secret` (file)        | `pcloud-secret` (file)        | `pcloud-secret` (file)        |
| **Supervisor**          | systemd (user + system)       | launchd (LaunchDaemon / LaunchAgent)    | Windows Service Control Manager (SCM)        | rc.d                          | rc.d                          | rc.d                          |
| **Config dir**          | `$XDG_CONFIG_HOME/pcloud-rs/` (`~/.config/pcloud-rs/`) | `~/Library/Application Support/pcloud-rs/` | `%APPDATA%\pCloud\`                          | `~/.config/pcloud-rs/`         | `~/.config/pcloud-rs/`         | `~/.config/pcloud-rs/`         |
| **Data dir**            | `$XDG_DATA_HOME/pcloud-rs/`    | `~/Library/Application Support/pcloud-rs/` | `%LOCALAPPDATA%\pCloud\`                     | `~/.local/share/pcloud-rs/`    | `~/.local/share/pcloud-rs/`    | `~/.local/share/pcloud-rs/`    |
| **Log dir**             | `journald` (syslog fallback)  | `~/Library/Logs/pcloud-rs/`              | Windows Event Log (Application channel)      | `/var/log/pcloud-rs/`          | `/var/log/pcloud-rs/`          | `/var/log/pcloud-rs/`          |
| **Clipboard**           | `wl-clipboard` / `xclip` via `arboard` | `NSPasteboard` via `arboard`    | Win32 clipboard via `arboard`                | X11 via `arboard` (optional)  | X11 via `arboard` (optional)  | X11 via `arboard` (optional)  |
| **Notification channel**| `notify-rust` → D-Bus (FDO)   | `UserNotifications` framework (bridged) | Windows Toast (WinRT)                        | D-Bus (FDO) if desktop        | D-Bus (FDO) if desktop        | D-Bus (FDO) if desktop        |
| **Signal handling**     | `SIGTERM`/`SIGINT` via `tokio::signal` | same; also launchd stop events   | `SetConsoleCtrlHandler` + SCM stop; named-pipe shutdown | POSIX signals                 | POSIX signals                 | POSIX signals                 |
| **Mount probe**         | `/proc/self/mountinfo`        | `getmntinfo(3)`, `struct statfs`        | `GetVolumeInformation` + `QueryDosDevice` + WinFSP control API | `getmntinfo(3)`               | `getmntinfo(3)`               | `getmntinfo(3)`               |

## Crate ownership

| Abstraction              | Crate                         | Platform-conditional sub-crates / features |
|--------------------------|-------------------------------|---------------------------------------------|
| Mount runtime            | `pcloud-fs`                   | `pcloud-fs-linux`, `pcloud-fs-mac` (fuse-t FFI), `pcloud-fs-win` (WinFSP), `pcloud-fs-bsd` |
| Local IPC                | `pcloud-ipc`                  | `cfg(windows)` named-pipe module; `cfg(unix)` AF_UNIX module |
| Peer-cred                | `pcloud-ipc::peer`            | Platform-specific implementations gated by `cfg`           |
| Secret vault             | `pcloud-secret`, `auth_vault` | `secret-service`, `security-framework`, DPAPI via `windows-sys` |
| Service supervision      | packaging assets (see [matrix](../operations/packaging-matrix.md)) | units ship with OS-native packages |

## Residual honest gaps

1. **Mount parity proof** (`bd-1du.4`): until fuse-t, WinFSP, and
   *BSD `fusefs` are live-tested on hardware, this document describes
   *scaffolded* capabilities for those targets — not verified ones.
2. **Notification / clipboard** on headless servers: degrades to no-op;
   tests assert graceful degradation, not feature parity.
3. **Vault fallback** on platforms without a native keyring (headless
   Linux, all *BSD): file-backed vault at `0600` with owner-only parent
   directory. Raw password persistence is **not** mirrored from the C
   client (see
   [ADR 0007](../adr/0007.md)).

## If you're new to platform support

The **thing to know**: the Rust codebase is intended to compile unchanged
on all T1 and T2 targets. Platform differences are concentrated in five
trait implementations (see [Overview](./overview.md) — "Five core platform
abstractions"), and nothing outside those traits may name an OS. When you
see a `#[cfg(target_os = "…")]` block in a non-platform crate, that is a
bug report.

Today, the honest status is:

- **Linux** is the daily-driver target and the only hardware-verified
  mount backend.
- **macOS** and **Windows** are scaffolded and compile green, but mount
  paths await hardware verification (`bd-1du.4`).
- **FreeBSD / OpenBSD / NetBSD** are best-effort T2/T3, IPC and packaging
  work, mount is untested on hardware.

## Per-platform deep walkthrough

### Linux (T1)

The mount backend uses `fuser`, which wraps libfuse3 in a safe Rust
binding. The daemon opens `/dev/fuse`, hands the fd to a worker thread,
and drives the FUSE wire protocol from
`crates/pcloud-fs/src/platform/linux.rs`. Unmount is a RAII `Drop` impl
on the mount handle that sends `fusermount3 -u` and waits for the kernel
mount to disappear from `/proc/self/mountinfo`.

IPC uses a Unix domain socket under `$XDG_RUNTIME_DIR/pcloud/ipc.sock`
(typically `/run/user/<uid>/pcloud/ipc.sock`). Peer cred comes from
`SO_PEERCRED`, which returns `struct ucred { pid, uid, gid }` captured at
the kernel level at accept time (so a later `setuid` by the peer does not
retroactively promote it). The parent directory is `0700`, the socket
`0600`; both are checked at accept time to defend against
`/tmp`-style replacement attacks.

The vault is `libsecret` via the Secret Service D-Bus API when a desktop
session is detected; otherwise it falls back to an integrity-checked
owner-only file under `$XDG_DATA_HOME/pcloud/auth_token`. The integrity
check uses BLAKE3; a tampered file is *not* recovered but treated as
absent, forcing a fresh login.

Supervisor: `systemd` user unit by default (`pcloudd.service`), or system
unit on headless servers. Signals are handled by `tokio::signal` only in
`pcloud-web`; the daemon uses `signal-hook-registry` to install a
SIGTERM/SIGINT handler that sets an `AtomicBool`.

### macOS (T1, hardware-verified for IPC and vault, mount scaffolded)

Mount uses `fuse-t`, a user-space FUSE compatibility layer that does
*not* require a kernel extension. The `pcloud-fs-mac` sub-crate loads
`libfuse-t.dylib` via FFI and drives the same callback surface as Linux
FUSE. The key difference is mount-point ownership: fuse-t mounts under
`/Volumes/pcloud-<uuid>` and registers with the Finder via the
standard `fuse-t` IPC.

IPC uses `AF_UNIX` under `~/Library/Application Support/pcloud-rs/ipc.sock`.
Peer cred is `LOCAL_PEERCRED` via `getsockopt` returning `xucred`;
comparison is against `geteuid()`. The socket permissions and parent
directory rules are identical to Linux.

Vault uses Keychain Services. The envelope is stored as a `kSecClassGenericPassword`
with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` so an iCloud
backup cannot lift the secret off-device. ACLs are scoped to the daemon's
bundle id; a re-signed binary triggers a user prompt rather than a silent
read.

Supervisor: `launchd`. A LaunchAgent runs the daemon in the user's login
session; a LaunchDaemon is used for device-scope deployments in the
enterprise plist.

### Windows (T1, IPC and vault verified, mount scaffolded)

Mount uses WinFSP 2.x via the `winfsp` crate. The mount point is either a
drive letter (`P:\`) or a directory junction
(`C:\Users\<user>\pCloud\`). The WinFSP user-mode service translates
Windows FS callbacks into the same per-file operations Linux FUSE
exposes; the adapter in `crates/pcloud-fs/src/platform/windows.rs`
normalises the differences (path separators, case-insensitive lookups,
ACL translation).

IPC uses a named pipe at `\\.\pipe\pcloud-<sid>`. The pipe security
descriptor is an explicit DACL built from `InitializeSecurityDescriptor`
plus `SetSecurityDescriptorDacl`; it grants `FILE_ALL_ACCESS` to exactly
two trustees (daemon SID, user SID) and denies inheritance. Peer check:
`GetNamedPipeClientProcessId` → `OpenProcessToken` → `TokenUser` → `EqualSid`.
On mismatch the pipe is disconnected before the first byte is read.

Windows 10 1803+ also supports `AF_UNIX`; the IPC module probes for this
and will use a Unix socket path under `%LOCALAPPDATA%\pcloud\ipc.sock`
when available, falling back to the named pipe on older builds.

Vault is DPAPI via `CryptProtectData` with `CRYPTPROTECT_UI_FORBIDDEN`.
The envelope is stored under `%APPDATA%\pcloud\auth_token`. DPAPI binds
the ciphertext to the user's logon credential, so a stolen disk image
from another account cannot recover the token.

Supervisor: Windows SCM. The service manifest is in
`packaging/windows/pcloudd-service.xml`.

### FreeBSD (T2), OpenBSD / NetBSD (T3)

Mount is scaffolded on top of `fusefs` (FreeBSD), `fusefs` (OpenBSD), and
`refuse` (NetBSD) via `fuser`. No hardware verification.

IPC is `AF_UNIX` with `getpeereid(3)` on all three; socket and directory
permissions are identical to Linux. Vault falls back to the integrity-checked
owner-only file path. Supervisor is `rc.d`.

## State machine: mount lifecycle across platforms

The state machine is identical on every platform; the per-platform
implementation lives in `crates/pcloud-fs/src/platform/<os>.rs`.

| From       | Event           | To          | Notes                                          |
|------------|-----------------|-------------|------------------------------------------------|
| Absent     | `mount()`       | Mounting    | Reject if mount point is already a mount.      |
| Mounting   | ready           | Online      | Journal replay runs before accepting IO.       |
| Mounting   | error           | Failed      | Cleanup partial kernel state.                  |
| Online     | `unmount()`     | Unmounting  | Drain in-flight writes under back-pressure.    |
| Online     | SIGTERM/SIGINT  | Unmounting  | Signal-aware RAII handle fires the same path.  |
| Online     | OOM/panic       | Unmounting  | Panic guard drives the unmount via `Drop`.     |
| Unmounting | success         | Absent      | `MountinfoReader` confirms absence.            |
| Unmounting | timeout         | Failed      | Operator intervention required.                |
| Failed     | `unmount()`     | Absent      | Force unmount (`umount -l` on Linux).          |

## Tradeoffs and design decisions

- **Why fuse-t on macOS instead of macFUSE?** macFUSE requires a signed
  kernel extension and third-party KEXT load policy. fuse-t runs entirely
  in user space, simplifies distribution, and works on Apple Silicon
  without kext loading. Cost: slightly higher per-IO overhead, which is
  acceptable for a network-bound client.
- **Why WinFSP instead of Dokan?** WinFSP has the stronger security story
  (explicit DACL on the mount, no kernel-mode token impersonation) and a
  more active maintenance cycle.
- **Why a Unix-socket fallback on Windows?** Because pipe DACLs are
  painful to audit at scale; an AF_UNIX socket under
  `%LOCALAPPDATA%` on Win10 1803+ gives us Unix-style permission checks
  and the same `SO_PEERCRED`-equivalent.
- **Why require the Secret Service D-Bus API on Linux desktops instead
  of rolling our own?** Because a broken desktop keyring is the user's
  problem, not ours, and forcing integration reveals environment issues
  early.

## Concurrency model (platform-specific)

- Linux: FUSE callbacks run on a dedicated `fuser` thread pool (bounded,
  sized to the number of cores); each callback posts into the daemon's
  sync request path via a `crossbeam_channel`.
- macOS: fuse-t callbacks arrive over a Unix socket from the fuse-t
  helper; the adapter owns a small dispatch thread that translates
  them into the same channel.
- Windows: WinFSP callbacks run on the WinFSP service's threadpool; the
  adapter translates them into IPC-style requests into the daemon.

Peer-check threads are always single-shot per connection on every
platform.

## Security invariants (platform-specific)

- Linux: `SO_PEERCRED` returns kernel-captured credentials; test at
  `crates/pcloud-ipc/tests/peer_cred_linux.rs`.
- macOS: `LOCAL_PEERCRED` returns kernel-captured `xucred`; test at
  `crates/pcloud-ipc/tests/peer_cred_macos.rs`.
- Windows: SID-DACL must deny `Everyone`; test at
  `crates/pcloud-daemon-win/tests/pipe_dacl.rs`.
- All platforms: vault file (when used) is `0600` on a `0700` parent;
  test at `crates/pcloud-daemon/tests/vault_perms.rs`.

## Extension points

- New platform: implement the five traits (`PlatformMount`,
  `PlatformIpc`, `PlatformVault`, `MountinfoReader`, `PcloudDirs`) in a
  new sub-crate under `crates/pcloud-fs-<os>` (for mount) and inside
  `pcloud-ipc` / `pcloud-secret` for the other four. Register the new
  target in the CI matrix.
- Alternative vault: implement `PlatformVault` against an enterprise KMS
  via `pcloud-kms::KmsProvider`.
- Alternative mount backend: implement `PlatformMount` against any FS
  binding that exposes readdir/open/read/write; the daemon dispatch does
  not care which backend it is driving.

## Open `bd` trackers

- **`bd-1du`** — parity epic.
- **`bd-1du.4`** — mounted-drive parity: proves fuse-t, WinFSP, and
  *BSD `fusefs` on hardware.
- **`bd-1du.4.6.1`** — enterprise readiness surface (KMS, IDP, policy).
- **`bd-1du.10`** — parity proof gates the release wording that lives in
  this matrix.

## Cross-references

- [Overview](./overview.md) for the five platform traits in context.
- [Crate Map](./crate-map.md) for which crates are platform-conditional.
- [Operations → Platforms](../operations/platforms/linux.md) for
  operator-facing platform info.
- [Operations → Packaging Matrix](../operations/packaging-matrix.md) for
  packaging-side ownership.
- [Security Model](./security-model.md) for the security invariants.
- [Request Lifecycle](./request-lifecycle.md) — the end-to-end platform
  variations section.

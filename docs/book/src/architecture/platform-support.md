# Platform support

This is the authoritative capability and qualification matrix for the native
library, CLI, daemon, IPC, secret vault, and mounted-drive adapters. Packaging
status is tracked separately in the
[packaging matrix](../operations/packaging-matrix.md).

> **Evidence rule (2026-07-16).** Source code and workflow definitions prove
> that a path exists; they do not prove that a native job or device test has
> passed. A platform may be advertised as supported only after its listed gate
> has completed successfully for the release commit. NAS packages remain Tier
> 2 candidates until the hardware matrix passes.

## Product tiers

- **Tier 1 target:** Linux, macOS, Windows, FreeBSD, NetBSD, OpenBSD,
  DragonFly BSD, illumos/OmniOS, and Oracle Solaris. The portable library,
  `RemoteFs`, CLI, daemon, transfer, sharing, and API surfaces are expected to
  work on every Tier 1 target.
- **Tier 1 mount target:** Linux, macOS, Windows, FreeBSD, NetBSD, OpenBSD,
  and DragonFly BSD.
- **Tier 1 without a kernel mount:** illumos and Solaris currently return
  `MountError::UnsupportedPlatform`. This is deliberate: `fuser 0.16` does not
  implement the Solaris-family mount/unmount ABI. API, CLI, copy, transfer,
  and share operations are not coupled to a kernel mount. The WebDAV crate is
  experimental and unshipped; its implemented subset has a daemon/`RemoteFs`
  IPC adapter but no compliance-class claim.
- **Tier 2 target:** Synology DSM, QNAP QTS/QuTS hero, and ASUSTOR ADM native
  packages. These use Linux binaries and a shared durable supervisor, but each
  appliance family requires its own hardware qualification.

## Capability matrix

| Target | Mount adapter | Local peer authentication | Default/automatic vault | Native qualification path |
|---|---|---|---|---|
| Linux | `fuser` / FUSE | AF_UNIX + `SO_PEERCRED` | Secret Service when available, owner-only file fallback | strict local `/dev/fuse` aggregate plus release-commit live mount/package gates |
| macOS | direct fuse-t FFI | AF_UNIX + `getpeereid(3)` | Keychain | hosted portable tests plus labelled fuse-t mount and signed/notarized package job |
| Windows 10/11 | direct WinFSP FFI | named pipe + exact TokenUser SID comparison | DPAPI | hosted named-pipe tests plus checksum-pinned WinFSP live mount and signed Burn/MSI job |
| FreeBSD | `fuser` / `fusefs` | AF_UNIX + `getpeereid(3)` | owner-only file vault | strict native VM workspace and live FUSE job |
| NetBSD | `fuser` / native FUSE device | AF_UNIX + `getpeereid(3)` | owner-only file vault | strict native VM workspace and live FUSE job |
| OpenBSD | `fuser` / `fusefs` | AF_UNIX + `getpeereid(3)` | owner-only file vault | strict native VM workspace and live FUSE job |
| DragonFly BSD | `fuser` / `fusefs` | AF_UNIX + `getpeereid(3)` | owner-only file vault | strict native VM workspace and live FUSE job |
| OmniOS/illumos | explicit unsupported mount | AF_UNIX + `getpeerucred(3)` | owner-only file vault | native OmniOS workspace/API/CLI job |
| Oracle Solaris 11.4 | explicit unsupported mount | AF_UNIX + `getpeerucred(3)` | owner-only file vault | native Solaris workspace/API/CLI job |
| Synology/QNAP/ASUSTOR | firmware-dependent `/dev/fuse`; never auto-mounted | package-local Unix IPC | package-local owner-only file vault | package validation plus per-vendor hardware matrix |

The table describes intended release gates. Consult the GitHub Actions result
for the exact commit before making a release claim.

The current local evidence is narrower than the target matrix. On 2026-07-16,
16 practical ignored mount/probe tests passed on Arch Linux x86_64 with a real
kernel FUSE device and left no mount behind. This did not authenticate against
a live pCloud account, install a package, or run from a clean release commit.
It is therefore kernel-adapter qualification evidence, not proof that Linux or
any other target is ready for public release.

## Why `RemoteFs` is platform-neutral

`pcloud_backends::RemoteFs` is the canonical ID-first facade for remote
folder, transfer, and share operations. It resolves paths through the live
remote API and treats the metadata cache only as an optimization. The CLI and
SDK reach this facade through daemon IPC, while sync and mount adapters consume
it inside the daemon. An empty or stale local cache therefore cannot create a
second interpretation of the remote tree. The experimental WebDAV crate is
intentionally demoted: its concrete adapter routes implemented verbs through
the daemon's `RemoteFs` IPC surface, but the listener is not bootstrapped or
shipped and makes no compliance claim.

This separation is important on platforms where a kernel mount is unavailable:
all production remote operations remain available through the library, CLI,
or API. The OS-specific mount adapter is only another consumer of `RemoteFs`;
it is not the owner of remote state.

## Crate ownership

| Concern | Owner | Platform seam |
|---|---|---|
| Canonical remote namespace and operations | `pcloud-backends::RemoteFs` | none; ID-first and portable |
| SDK facade | `pcloud-sdk` | portable wrapper over `RemoteFs` |
| CLI | `pcloud-cli` | portable IPC client |
| Experimental WebDAV adapter | `pcloud-webdav::RemoteFsIpcBackend` | portable owner-authenticated IPC client; listener unshipped |
| Kernel mount | `pcloud-fs::platform` | `LinuxPlatformMount`, `MacosPlatformMount`, `WindowsPlatformMount`, `BsdPlatformMount`, or `UnsupportedPlatformMount` |
| Mount discovery/orphan cleanup | `pcloud-fs::MountinfoReader` | `/proc/self/mountinfo`, `getmntinfo(3)`, or Windows volume APIs |
| Local IPC | `pcloud-ipc::platform` | `SO_PEERCRED`, `getpeereid`, `getpeerucred`, or named-pipe SID |
| Secret storage | `pcloud-daemon::vault` | Secret Service/file, Keychain, DPAPI, or file |
| Service lifecycle | `packaging/` and CLI | systemd, launchd, per-user Windows start, rc.d, or NAS package hooks |

There are no `pcloud-fs-mac`, `pcloud-fs-win`, or `pcloud-fs-bsd`
sub-crates. Platform implementations live under
`crates/pcloud-fs/src/platform/` and are selected at compile time.

## Mount lifecycle

All implemented adapters uphold the same lifecycle:

| From | Event | To | Required behavior |
|---|---|---|---|
| Absent | `mount()` | Mounting | validate path and reject an existing mount |
| Mounting | native backend ready | Online | replay durable journal before accepting writes |
| Mounting | error | Failed | release all partially initialized native state |
| Online | `unmount()` or service stop | Unmounting | stop new work and drain in-flight writes |
| Unmounting | success | Absent | native mount discovery confirms removal |
| Unmounting | timeout/error | Failed | return a surfaced error; never claim success |

RAII handles own ordinary teardown. Linux/BSD signal handling, macOS fuse-t
cleanup, and the Windows active-mount reaper provide bounded abnormal-stop
paths. A process kill or kernel failure can still require native recovery
tooling; release qualification must exercise these cases.

## Platform details

### Linux

Linux uses `fuser`, `/dev/fuse`, `/proc/self/mountinfo`, AF_UNIX sockets with
kernel `SO_PEERCRED`, and a systemd user or system unit. `VaultBackend::Auto`
uses Secret Service when a session service is reachable and otherwise returns
an explicit warning while selecting the owner-only file vault.

### macOS

macOS loads fuse-t through direct FFI and discovers mounts with
`getmntinfo(3)`. IPC uses `getpeereid(3)`, not Linux `SO_PEERCRED` or
`LOCAL_PEERCRED`. Automatic token storage uses Keychain. The public package
job is intentionally strict: it requires Apple credentials, passes native
fuse-t read/write/unmount tests, signs the binaries and installer, notarizes,
staples, and assesses the resulting package. The installed per-user
LaunchAgent is materialized without running the daemon as root.

### Windows

Windows uses WinFSP through a small audited direct-FFI layer. Local IPC is an
owner-specific named pipe whose DACL and accepted client TokenUser SID must
match the daemon owner. There is no AF_UNIX fallback. Automatic token storage
uses user-scope DPAPI. The public installer job requires signing credentials,
signs binaries and the MSI, verifies a pinned vendor-signed WinFSP MSI, and
produces a final signed WiX Burn bootstrapper.

### BSD family

FreeBSD, NetBSD, OpenBSD, and DragonFly BSD share the `BsdPlatformMount` and
`getpeereid(3)` IPC implementation while using their native FUSE device and
mount table. Each explicitly supported BSD has its own strict native VM gate
and in-tree rc.d asset. The DragonFly job additionally builds and retains a
deterministic native binary/service candidate. These assets still require a
successful release-commit run and native install/upgrade testing.

### illumos and Solaris

The portable library, daemon, SDK, CLI, and IPC compile for these targets.
`SolarishIpc` uses `getpeerucred(3)` and frees the returned credential object
after extracting effective UID and PID. Kernel mounting is deliberately
unsupported until a native adapter with a correct mount and unmount ABI lands.
The native CI jobs are portability gates, not mount gates. Both jobs validate
the in-tree SMF definition, build release binaries, assemble deterministic
native candidates, and retain them as workflow artifacts.

### NAS appliances

Synology, QNAP, and ASUSTOR packages contain the same `pcloudc` and `pcloudd`
binaries plus a common supervisor. They do not auto-mount, do not run the
daemon as root to bypass FUSE permissions, and keep state in the vendor's
persistent package directory. See `packaging/nas/README.md` for the hardware
matrix and package-specific roots.

## Remaining release blockers

- The development worktree must be separated into reviewable commits and all
  gates repeated from a clean release candidate. A dirty local pass is not a
  reproducible release baseline.
- Linux still needs release-commit CI, install/upgrade/uninstall testing of the
  actual packages, and a credentialed pCloud transfer/share/mount smoke test.
- The focused `pcloud-sdk` source contract is version 1.0.0 and packageable
  after its registry dependencies exist. It has been split from the broad
  unpublished `pcloud-embedded-sdk` compatibility API and exposes only
  SDK-owned types over daemon IPC. No stable SDK release has been published;
  the required registry order is `pcloud-model`, `pcloud-ipc`, then
  `pcloud-sdk`, followed by install-from-registry verification.
- A newly added native workflow is not passing evidence until it has run on
  the release commit.
- macOS and Windows public packages require real signing/notarization secrets
  and successful native jobs.
- BSD and Solaris-family service/package candidates still need retained
  release-commit runs plus native install, start/stop, upgrade, and uninstall
  evidence; no downstream ports, pkgsrc, or IPS repository is published yet.
- illumos/Solaris kernel mounts remain explicitly unsupported.
- Every NAS family needs install, upgrade, start/stop, reboot,
  uninstall/reinstall, and live transfer testing on representative hardware.

These gaps must remain visible in release notes and marketing copy; none may
be converted into a support claim by documentation alone.

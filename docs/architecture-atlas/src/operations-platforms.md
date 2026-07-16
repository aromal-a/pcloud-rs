# Operations and platform architecture

## Runtime lifecycle

```text
install package
   │
   ├── place pcloudc + pcloudd + service assets
   ├── create/select user-scoped state locations
   └── configure native mount dependency where applicable
        ▼
start pcloudd serve
   │ bootstrap → store/vault → IPC → background services
   ▼
login / operate / sync / mount
   ▼
graceful stop
   │ drain → persist → unmount → remove endpoint
   ▼
upgrade or uninstall
```

Do not run the daemon as root merely to bypass mount permissions. Public
Windows operation is per-user `pcloudd.exe`, not the experimental SCM host.
NAS packages use a package-local supervisor and persistent package state.

## Platform capability versus evidence

| Target family | Portable CLI/API | Native IPC/vault seam | Kernel mount | Current qualification interpretation |
|---|---|---|---|---|
| Linux | intended Tier 1 | AF_UNIX, `SO_PEERCRED`, Secret Service/file | FUSE | strict local kernel tests exist; clean release/package/credential gates still required |
| macOS | intended Tier 1 | `getpeereid`, Keychain | fuse-t | native signed/notarized package and mount qualification required |
| Windows 10/11 | intended Tier 1 | named pipe + SID, DPAPI | WinFSP | native signed installer and mount qualification required |
| FreeBSD/NetBSD/OpenBSD/DragonFly | intended Tier 1 | `getpeereid`, file vault | native FUSE family | per-BSD native release/package/mount gates required |
| illumos/OmniOS/Solaris | intended portable Tier 1 | `getpeerucred`, file vault | explicitly unsupported | native API/CLI/service/package gates required; no mount claim |
| Synology/QNAP/ASUSTOR | Tier 2 candidate | package-local Unix IPC/file vault | firmware-dependent, no auto-mount | representative hardware matrix required |

This table is architectural intent, not a release certificate. Check the
release commit's native job and artifact evidence before advertising support.

## State locations

Exact paths are selected by configuration/platform helpers, but the logical
separation is:

```text
config root       profiles and operator configuration
data root         SQLite store, durable application state
runtime root      owner-only IPC endpoint, short-lived journals/locks
cache root        safely rebuildable cache data
mount staging     bounded writeback state, preferably per mount
```

Permissions and ownership are validated, especially for vault, runtime, and
database paths. Package scripts must preserve durable data across upgrades and
must not leave active mounts or service users behind on uninstall.

## Packaging ownership

The `packaging/` tree contains:

- Debian/nfpm, AppImage, Flatpak, Snap, Docker, Homebrew, Chocolatey, Scoop,
  WinGet, WiX/Burn, macOS pkg/dmg, generic Unix tarball;
- systemd, launchd, rc.d, OpenRC, runit, s6, dinit, SysV and SMF assets;
- SELinux and AppArmor policy;
- Synology, QNAP and ASUSTOR builders/supervisor;
- signing, notarization, and reproducibility helpers.

The generated [packaging inventory](generated/inventory/packaging.md)
documents every visible packaging file.

## Operational observability

Operators should distinguish:

- liveness: process and IPC endpoint exist;
- readiness: store/migrations/runtime are usable and required auth state is
  established;
- transfer state: active, paused, recoverable, conflicted, or drained;
- mount state: native discovery confirms online/absent;
- integrity: journal replay and audit-chain verification succeeded.

Metrics and health types live in `pcloud-observability`, daemon metrics
service code, and IPC health responses. The web UI is an optional view, not a
replacement for daemon health semantics.

## Native release checklist

For each advertised target:

1. build on the native release commit with locked dependencies;
2. run workspace and platform-specific tests;
3. install the actual artifact on a clean system;
4. start, login, copy/upload/download/share, stop and restart;
5. mount/read/write/fsync/unmount where mounting is claimed;
6. upgrade from the previous supported artifact;
7. uninstall and verify no daemon, endpoint, or mount leaks;
8. verify signatures/notarization where applicable;
9. retain logs, checksums, and artifact provenance.

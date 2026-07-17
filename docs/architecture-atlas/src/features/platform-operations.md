# Platforms, packaging, and operations

Platform support has three independent layers:

1. **Portable product:** model, protocol, daemon, CLI, SDK, RemoteFs,
   transfers, sharing, Crypto, and local state.
2. **Native integration:** peer-authenticated IPC, credential vault, kernel
   mount, signals/services, filesystem discovery, and UI notifications.
3. **Distribution evidence:** package build/install/upgrade/uninstall,
   signing/notarization, live account work, native mount, and retained results.

Source code can prove layers 1–2 exist. Only a native release-commit run and
real artifact/device tests qualify layer 3.

## Platform capability matrix

| Platform | Portable/API features | IPC identity | Vault | Mount | Service/package path | Status truth |
|---|---|---|---|---|---|---|
| Linux | Full intended portable surface | AF_UNIX + kernel `SO_PEERCRED` UID | Secret Service when available; owner-only file fallback | `fuser` + `/dev/fuse`, mountinfo | systemd user/system, deb/RPM, tar, AppImage/Flatpak/Snap, Docker; AppArmor/SELinux assets | Primary development path; release still requires clean artifact install and credentialed/native gates |
| macOS | Full intended portable surface | AF_UNIX + `getpeereid(3)` | Keychain | direct fuse-t/macFUSE-shaped FFI and `getmntinfo` | per-user launchd, PKG/DMG/Homebrew, signing/notarization assets | Source implemented; real Mac mount, codesign, notarization, Gatekeeper, install/upgrade/uninstall required |
| Windows 10/11 | Full intended portable surface | named pipe + exact client TokenUser SID and DACL | Current-user DPAPI with secure file handling | direct WinFSP FFI + volume discovery/drive letter | per-user daemon, WiX MSI/Burn WinFSP bundle, winget/Chocolatey/Scoop; optional experimental SCM wrapper | Native remote CI path exists; signed installer/DPAPI/WinFSP and interactive identity must pass for release |
| FreeBSD | Full intended portable surface | AF_UNIX + `getpeereid(3)` | owner-only file | `fuser`/native fusefs | rc.d and Unix tar candidate | Source target; native build, FUSE, install/upgrade/reboot/uninstall evidence required |
| NetBSD | Full intended portable surface | AF_UNIX + `getpeereid(3)` | owner-only file | `fuser`/native FUSE | rc.d and Unix tar candidate | Same evidence requirement, tested separately because device/mount behavior differs |
| OpenBSD | Full intended portable surface | AF_UNIX + `getpeereid(3)` | owner-only file | `fuser`/fusefs | rc.d and Unix tar candidate | Same evidence requirement; pledge/unveil/package policy is not inferred from generic Unix code |
| DragonFly BSD | Full intended portable surface | AF_UNIX + `getpeereid(3)` | owner-only file | `fuser`/fusefs | dedicated rc.d and deterministic Unix candidate | Dedicated cfg/vendor fixes exist; native release installation and mount evidence still required |
| illumos/OmniOS | CLI/API/SDK/daemon/transfer/share intended | AF_UNIX + `getpeerucred(3)` | owner-only file | Explicitly unsupported | SMF + deterministic Unix candidate | Tier-1 portable target without kernel mount; native runtime/package evidence required |
| Oracle Solaris 11.4 | CLI/API/SDK/daemon/transfer/share intended | AF_UNIX + `getpeerucred(3)` | owner-only file | Explicitly unsupported | SMF + deterministic Unix candidate | Same: no mount claim until a real ABI adapter exists |
| Synology DSM | Linux portable binary in vendor package | package-local Unix endpoint | package-local owner-only file | firmware/model-dependent, never auto-enabled | SPK scripts/privilege metadata + shared supervisor | Tier-2 candidate; representative DSM architecture/model hardware matrix mandatory |
| QNAP QTS/QuTS hero | Linux portable binary in vendor package | package-local Unix endpoint | package-local owner-only file | firmware/model-dependent, never auto-enabled | QPKG/QDK metadata and start script + shared supervisor | Tier-2 candidate; QTS and QuTS hero hardware/install lifecycle required |
| ASUSTOR ADM | Linux portable binary in vendor package | package-local Unix endpoint | package-local owner-only file | firmware/model-dependent, never auto-enabled | APKG `.apk`, config/icon/start-stop + shared supervisor | Tier-2 candidate; official tooling and ADM model tests required |

## Tier-1 OS lifecycle playbooks

These are qualification playbooks for the repository's package candidates,
not claims that a public package repository exists. Execute the whole
lifecycle on the exact release artifact and retain the result. Before any
upgrade or uninstall, drain/unmount first and make a consistent copy of the
state directory and auth vault. Restoring an older binary against a
forward-migrated database is not a valid rollback unless that schema path was
explicitly tested.

### Linux

| Stage | Procedure and pass condition |
|---|---|
| Install | Install the exact `.deb`, RPM, or deterministic Unix candidate, or build `pcloudd` and `pcloudc` with `--release --locked`. No project APT/YUM repository is currently promised. Verify binaries, man pages, service assets, owner-only state/runtime paths, and dynamic-library/FUSE dependencies. |
| Start and observe | For an interactive user, use `pcloudc start` or install `packaging/systemd/pcloudd-user.service`, then run `systemctl --user enable --now pcloudd` and `pcloudc status`. A system unit runs under its service identity; authentication and the vault must use that same identity. Inspect with `systemctl ... status` and `journalctl`. |
| Mount | Install FUSE3, prove `/dev/fuse` access, install the FUSE systemd drop-in only when required, then exercise mount, create/write/fsync/read/rename/delete, unmount, and orphan cleanup. `fusermount3 -u -z` is recovery tooling, not the normal close path. |
| Upgrade | Run `pcloudc drain` for a per-user Unix daemon (or stop the owning systemd service), verify no mount/process remains, snapshot state, install the replacement, run `daemon-reload`, restart, and prove status, login/vault continuity, transfer resume, share, and mount as applicable. Test failed-upgrade rollback separately. |
| Recovery | Run `pcloudc doctor`, inspect unit logs and state permissions, check pending upload/write journals, and clean only a confirmed stale mount. Restart must replay recoverable work without publishing a partial final file or losing the auth-vault scope. |
| Uninstall | Stop/disable the user or system unit, unmount, remove the package/binaries/unit/drop-ins, reload systemd, and verify no process/socket/mount remains. Package scripts intentionally do not silently delete per-user state; test explicit retain-data and remove-data procedures. |
| Release evidence | Retain clean install, first login, API transfer/share, restart/crash resume, mount when claimed, in-place upgrade, purge, and reinstall results for each advertised distribution/architecture. |

### macOS

| Stage | Procedure and pass condition |
|---|---|
| Install | Verify the signed/notarized PKG with `pkgutil`/`spctl`, install fuse-t when mount is claimed, run `installer -pkg ... -target /`, then run `/usr/local/share/pcloud-rs/macos/configure-user.sh` **without sudo**. The package installs binaries globally but the supported interactive daemon is the user's LaunchAgent. |
| Start and observe | `configure-user.sh` bootstraps and kickstarts `gui/<uid>/com.pcloud.pcloud-rs`; check it with `launchctl print` or `packaging/macos/launchd-status.sh`, then run `pcloudc status`. Keychain, IPC, and Finder mount must all remain in the same GUI-user identity. |
| Upgrade | Run `pcloudc drain`, unmount, boot out the LaunchAgent, preserve state/Keychain access, install the new signed PKG, rerun `configure-user.sh`, and prove vault continuity, API calls, launch-at-login, sleep/wake, and fuse-t read/write/fsync/unmount. Reject downgrade unless database compatibility is proven. |
| Recovery | Use `launchd-status.sh`, `launchctl print`, and the per-user logs; validate the plist, runtime directory, socket ownership, Keychain item, and fuse-t installation. Exercise abnormal-stop journal replay and stale-mount cleanup without deleting state first. |
| Uninstall | Run `packaging/macos/uninstall.sh` from the affected user, confirm LaunchAgent bootout and binary/plist removal, and verify no mount/process remains. The script deliberately preserves user data and lists its locations; test both retention and a separately authorized clean-data removal. |
| Release evidence | Retain arm64 and x86_64 evidence where advertised: codesign, notarization/stapling, Gatekeeper, clean PKG install, login, Keychain, launchd, mount, upgrade, uninstall, and clean-host reinstall. A Homebrew/DMG route is a separate channel and needs its own lifecycle. |

### Windows 10/11

| Stage | Procedure and pass condition |
|---|---|
| Install | Verify Authenticode, then install the signed Burn `-setup.exe`, which chains the checksum-pinned WinFSP MSI. The standalone MSI must refuse a fresh install without WinFSP. Binaries land below `Program Files`; no machine-wide SCM service is installed. |
| Start and observe | Run `pcloudc start` and `pcloudc status` as the interactive user. Prove the named pipe DACL/client SID, CurrentUser DPAPI vault, and WinFSP mount all use that same SID. Test standard-user operation after the elevated installer exits. |
| Upgrade | Use `pcloudc stop` on Windows, unmount, snapshot state, and run the signed major-upgrade bundle/MSI. Verify the stable WiX `UpgradeCode`, no duplicate installation, state/DPAPI continuity under the same user, transfer recovery, and WinFSP compatibility. Test repair and interrupted-upgrade behavior. |
| Recovery | Run `pcloudc doctor`/`status`, inspect the interactive user's logs/state, enumerate WinFSP volumes, and verify named-pipe and DPAPI identity before resetting anything. A service-account or administrator launch is not a valid recovery if it changes SID/vault scope. |
| Uninstall | Uninstall through Settings or `msiexec /x ...`; first stop/unmount and decide whether state is retained. Verify application files/shortcuts disappear, no daemon/pipe/volume remains, and the shared WinFSP prerequisite is handled according to bundle policy rather than removed blindly. |
| Release evidence | Retain signed bundle/MSI install, UAC boundary, standard-user first run, named-pipe hostile-user check, DPAPI restart, WinFSP read/write/unmount, repair, upgrade, reboot, uninstall, and reinstall results on each advertised Windows release. |

### FreeBSD

| Stage | Procedure and pass condition |
|---|---|
| Install | Build with `--release --locked`, install binaries and the wrapper, create the unprivileged `pcloudd` identity and `0700` state, and install `packaging/freebsd/pcloudd.rc` under rc.d. Install/load `fusefs` and enable user mounts only when mounting is claimed. This is a package candidate, not a published port. |
| Start/upgrade | Enable with `sysrc pcloudd_enable=YES`; exercise `service pcloudd start|status|stop`. For upgrade, stop/drain, snapshot state, replace the candidate, start, and prove schema/vault/journal continuity. The `daemon(8)` supervisor PID must not be confused with a stale child PID. |
| Recovery/uninstall | Inspect rc.d status, syslog, `/var/run/pcloudd.pid`, state ownership, and `/dev/fuse`; prove SIGTERM and journal replay. Disable/stop, unmount, remove rc.d/wrapper/binaries, and test explicit state retain/removal. Retain native package install/upgrade/reboot/uninstall evidence for the named FreeBSD version and architecture. |

### NetBSD

| Stage | Procedure and pass condition |
|---|---|
| Install | Build locked release binaries, create the service identity/state, install the wrapper and `packaging/netbsd/pcloudd` into the native rc.d layout, and set `pcloudd=YES` in rc.conf. Verify the native `/dev/puffs` or `/dev/fuse` path when mount is claimed. No pkgsrc package is currently promised. |
| Start/upgrade | Exercise `/etc/rc.d/pcloudd start|status|stop`; the wrapper `exec`s the child so the recorded PID remains authoritative. Stop, snapshot, replace, restart, and verify vault/state migration plus journal and mount recovery on upgrade. |
| Recovery/uninstall | Check the rc.d PID/log, service-user permissions, device access, IPC peer identity, and stale mounts before changing state. Disable/stop, unmount, remove service/wrapper/binaries, and separately choose state retention. Retain the full lifecycle on the exact NetBSD version/architecture advertised. |

### OpenBSD

| Stage | Procedure and pass condition |
|---|---|
| Install | Build locked release binaries, create the `_pcloud` identity and `0700` state, install the wrapper/environment and `packaging/openbsd/pcloudd` as `/etc/rc.d/pcloudd`, and verify `/dev/fuse0` only for mount qualification. This is not yet a published ports package. |
| Start/upgrade | Use `rcctl enable pcloudd`, `rcctl start pcloudd`, `rcctl check pcloudd`, and `rcctl stop pcloudd`. The script's `rc_bg=YES` and `pexp` own foreground-process matching. Stop/drain, snapshot, replace, restart, and prove IPC/vault/journal/mount behavior. |
| Recovery/uninstall | Inspect `rcctl`/daemon logs, `_pcloud` ownership, process matching, and FUSE state; prove abnormal-stop replay. Disable/stop, unmount, remove rc.d/wrapper/binaries, and explicitly retain or remove state. Qualify pledge/unveil or package-policy changes rather than inferring them from generic Unix support. |

### DragonFly BSD

| Stage | Procedure and pass condition |
|---|---|
| Install | Use the deterministic Unix candidate, create `pcloudd`, install the wrapper/environment and `packaging/dragonfly/pcloudd` under `/usr/local/etc/rc.d`, and provision `0700` state. Install `fusefs-libs3` and verify the native device when mount is claimed; no dport package is promised. |
| Start/upgrade | Enable with `sysrc pcloudd_enable=YES` and exercise `service pcloudd start|status|stop`. DragonFly `daemon(8)` owns the locked supervisor PID, bounded restart, privilege drop, and SIGTERM forwarding. Stop, snapshot, replace, restart, and prove state/journal/mount continuity. |
| Recovery/uninstall | Inspect syslog, `/var/run/pcloudd.pid`, service identity, device, and mount table; prove restart does not duplicate committed work. Disable/stop, unmount, remove installed assets, and separately retain/remove state. Retain install, upgrade, reboot, drain, and uninstall evidence on the advertised native release. |

### illumos and OmniOS

| Stage | Procedure and pass condition |
|---|---|
| Install | Build on the exact OmniOS/illumos target, create `pcloudd` and `0700` state, install the wrapper, `packaging/solarish/pcloudd` method, and `pcloudd.xml` manifest, then run `svccfg validate` and manifest import. The candidate is not an IPS repository. Kernel mount is explicitly unsupported. |
| Start/upgrade | Enable and inspect `svc:/site/pcloud-rs:default` with `svcadm enable` and `svcs`. For upgrade, disable/stop, snapshot state, replace binaries/method/manifest, validate/import, enable, and prove AF_UNIX `getpeerucred`, file vault, API/copy/share, and journal behavior. |
| Recovery/uninstall | Use `svcs -xv` and `svcs -L`, validate the method environment/state ownership, and refresh/restart without inventing a mount fallback. Disable, `svccfg delete` the service when removing it, remove assets, and explicitly retain/remove state. Retain evidence on each advertised OmniOS/illumos release and architecture. |

### Oracle Solaris 11.4

| Stage | Procedure and pass condition |
|---|---|
| Install | Build and test in the Solaris 11.4 GCC/rustup environment, provision the dedicated identity/state, and install/validate the same standard SMF method/manifest paths. Do not describe the tar candidate as an IPS package. Mount requests must return the documented unsupported-platform error. |
| Start/upgrade | Import/enable `svc:/site/pcloud-rs:default`, prove service logs/status and portable API operations, then run a stop/snapshot/replace/validate/import/start upgrade. Verify `getpeerucred` object cleanup, owner-only vault, transfer/share, reload, and graceful SIGTERM behavior. |
| Recovery/uninstall | Diagnose with `svcs -xv`/`svcs -L`, repair permissions/config without deleting durable state, and prove journal recovery. Disable/delete the SMF service, remove method/manifest/binaries, and test explicit state retention/removal. Solaris evidence is separate from OmniOS evidence even though assets are shared. |

## Native integration rationale

### Local IPC

The daemon holds account authority, so “a local process can reach the socket”
is not sufficient authentication. Linux reads `SO_PEERCRED`; BSD/macOS use
`getpeereid`; Solaris-family code uses and frees `getpeerucred`; Windows
creates an owner-scoped named pipe and compares the accepted client's token
SID. Runtime directories/socket modes/DACLs provide a first barrier, and peer
identity provides a second. This is good for per-user CLI/SDK/web clients and
prevents another logged-in OS user from controlling the authenticated daemon.

### Token vaults

Auto selection prefers Keychain on macOS, DPAPI on Windows, Secret Service on
Linux, and the owner-only file vault on BSD/Solaris-family systems. Only auto
may fall back, and it returns a warning; an explicitly requested unavailable
backend fails. This makes deployment intent visible and avoids a silent
security downgrade. Passwords and Crypto keys are never persisted by this
vault contract.

### Kernel mount

Linux/BSD use the `fuser` integration; macOS and Windows use narrow direct
native FFI. All feed the same portable inode/read/write/journal/writeback
core, so a platform adapter cannot create a different remote filesystem.
Mount is optional: unsupported Solaris-family mounting does not remove CLI,
SDK, copy, transfer, share, or other portable features.

### Signals and services

Unix signals trigger reload, drain, and shutdown through daemon-owned state.
systemd, launchd, rc.d, SMF, NAS hooks, and Windows launcher/installer assets
own process lifecycle but do not bypass graceful drain. The experimental
`pcloudd-svc` SCM wrapper is isolated because Windows service identities and
desktop/current-user vault/mount resources differ from the normal per-user
product.

## Files and state locations

| State class | Why it exists | Security/durability rule |
|---|---|---|
| Configuration | Versioned operator/user policy | Reject unsafe permissions/schema/cross-field combinations; no credentials in config |
| Runtime endpoint/pid | Short-lived IPC and process coordination | Owner-only directory; removed/verified at clean shutdown |
| SQLite state | Sync graph, cursors, metadata, resume, audit, settings | Transactions, migrations, WAL/integrity, backup before risky upgrade |
| Auth vault | Optional durable pCloud token | Explicit opt-in, native user scope or `0600` file, idempotent clear |
| Upload journal/resume | In-flight transfer durability | Persist acknowledgment before claiming progress; clear only after commit |
| Mount staging/journal | Kernel-write durability | Disk-backed staged image + ordered replay; drain/unmount before removal |
| Cache | Performance only | Bounded/evictable; never remote truth; optional sealed blobs for disk content |
| Logs/audit | Diagnosis and tamper-evident activity history | Redaction, rotation, stable categories; audit verification does not replace off-host retention |
| Snapshot/DR | Point-in-time recovery bundle | Manifest/hash/database consistency, optional GPG, restore drill and key recovery |

`pcloud-config::paths` translates these classes into platform-appropriate
directories and supports explicit roots for tests/multi-instance isolation.
Packagers must not invent a second layout without mapping every class.

## Packaging features

| Packaging family | Why it exists / good for | What the repository provides | Qualification still needed |
|---|---|---|---|
| Portable Unix tarball | Lowest-common-denominator binary/service delivery. | Deterministic tar builder, manifest/checksum validation, install template. | Native extraction/start/upgrade/uninstall on each target |
| Debian/RPM | Native Linux package lifecycle and dependencies. | `cargo-deb`/nfpm/control, postinst/postrm, logrotate, service assets. | Clean package build, install/upgrade/purge, service and FUSE/live smoke |
| systemd | User or system service supervision. | Separate user/system units, socket file, override examples. | The daemon owns its listener; socket activation is not claimed without explicit support |
| AppArmor/SELinux | Constrain daemon filesystem/process access. | Example policy/profile assets. | Distribution-specific compile/load, denial audit, mount/vault/network behavior |
| AppImage | Single-file Linux desktop distribution. | AppRun, desktop metadata, local build script. | Version/hash/update/signing and native execution tests |
| Flatpak/Snap | Sandboxed Linux channels. | Manifests/desktop metadata. | Store publication, confinement portals, FUSE/IPC/vault semantics, real install tests |
| Docker/Compose | Headless service/container portability smoke. | Dockerfile, entrypoint, compose, local xtask stage. | Image registry/signing/SBOM/runtime volume/secret policy; container is not a desktop mount substitute |
| macOS PKG/DMG/Homebrew | Native install and launchd integration. | Builders, install/uninstall/first-run, launchd plists, entitlements, Keychain setup, signing/notarization scripts, formula. | Real Apple credentials, notarized/stapled artifact, fuse-t, upgrade/uninstall and clean-host tests |
| Windows MSI/Burn | Installs binaries and a pinned WinFSP prerequisite. | WiX MSI/bundle, license, signing script, checksum/vendor verification design. | Authenticode credentials/timestamp, DPAPI/current-user behavior, install/repair/upgrade/uninstall/reboot and WinFSP mount |
| winget/Chocolatey/Scoop | Community Windows distribution metadata. | Channel manifests/scripts. | Real release URLs/hashes, submission, clean-host lifecycle and signature policy |
| BSD rc.d | Native daemon lifecycle on each BSD. | Per-OS scripts and common init assets. | Native package/ports/pkgsrc integration and service lifecycle |
| SMF | illumos/Solaris service management. | Manifest and launcher. | `svccfg` validation/install/upgrade/restart/uninstall on native hosts |
| Synology SPK | DSM Package Center-style candidate. | builder, privileges, scripts, metadata, shared supervisor. | DSM 7 models/architectures, persistence, reboot, migration, uninstall, live pCloud and optional FUSE |
| QNAP QPKG | QTS/QuTS package candidate. | QDK builder/config/start script, shared supervisor. | Both firmware families and representative hardware lifecycle/live tests |
| ASUSTOR APKG | ADM package candidate. | `.apk` builder/config/description/icons/start-stop, shared supervisor. | Official tooling and hardware lifecycle/live tests |
| Man pages | Offline operator/CLI/config reference. | `pcloudd(1)`, `pcloudc(1)`, `pcloud.conf(5)`. | Install paths, version consistency, and generation/update discipline |

A recipe is a feature for packagers, not proof of an available public channel.
No package should be marketed until the exact artifact from the release commit
passes the stated lifecycle.

## NAS appliance behavior

NAS packages use the same portable daemon/CLI and a shared durable supervisor,
but each vendor owns different package roots, lifecycle APIs, users/groups,
CPU families, libraries, and firmware. They therefore remain separate Tier-2
qualification targets.

Common rules:

- do not run as root merely to obtain `/dev/fuse`;
- do not auto-mount; API/CLI/backup/share features must work without FUSE;
- store config, database, token vault, journals, and logs in persistent
  package-approved paths;
- support install, upgrade, start/stop/status, reboot, uninstall/reinstall,
  crash recovery, storage-volume moves where supported, and log rotation;
- test live upload/download/copy/share and, only on models exposing safe FUSE,
  mount/write/read/fsync/unmount/orphan cleanup;
- test each distributed architecture and minimum firmware, not just archive
  extraction on x86 Linux.

### Synology DSM lifecycle

| Stage | Vendor-specific procedure and qualification |
|---|---|
| Build/install | Build one DSM 7 SPK per accepted `x86_64` or `armv8` payload with `build-spk.sh`, then run `packaging/nas/validate.sh`. Install the exact SPK through Package Center/manual upload on a representative model. Verify `INFO`, privilege metadata, package ownership, executable ABI, and `${SYNOPKG_PKGVAR}/root` plus `log` at mode `0700`. |
| Start/status/stop | DSM invokes `scripts/start-stop-status`; it maps vendor paths into `PCLOUD_PACKAGE_DIR`, `PCLOUD_DATA_DIR`, run/log directories, and the shared supervisor. Exercise Package Center start/restart/stop plus direct status. Pass only if PID validation, owner-only runtime files, `pcloudc status`, bounded drain, and log placement all agree. |
| Upgrade | Install a higher-version SPK over the running package after a controlled stop/drain. Prove DSM preserves `${SYNOPKG_PKGVAR}`, store migrations are one-time/transactional, the file vault remains usable by the package identity, and journals resume. The current candidate has no dedicated pre/post-upgrade migration script, so DSM behavior must be demonstrated rather than assumed. |
| Recovery | Kill the daemon during a staged transfer, reboot DSM, and prove supervisor stale-PID handling, state/vault permission retention, journal replay, and no leaked remote/temp object. Test a moved/replaced storage volume if the model supports it. FUSE recovery is separate and only applies when the exact model safely exposes `/dev/fuse`. |
| Uninstall/reinstall | Test both Package Center uninstall with retained operator backup and a deliberate remove-data path. The package has no custom uninstall/migration hook that guarantees retention; record exactly what DSM removes under `/var/packages/pcloud-rs`. Reinstall must either adopt a verified retained state or start clean, never a partial mixture. |
| Hardware evidence | Cover the minimum DSM release, each shipped CPU architecture, at least one low-memory model, upgrade/reboot, live login/upload/download/copy/share, log rotation, and optional mount. Archive inspection on Linux is only a builder test. |

### QNAP QTS and QuTS hero lifecycle

| Stage | Vendor-specific procedure and qualification |
|---|---|
| Build/install | Use QDK 2.5.3+ `qbuild` through `build-qpkg.sh` for `x86_64` and/or `arm_64`, then install the exact QPKG through App Center/manual install on both QTS and QuTS hero when both are advertised. Verify `/etc/config/qpkg.conf` resolves `Install_Path`, the selected volume is correct, and the staged binary ABI/libraries match the device. |
| Start/status/stop | `QPKG_SERVICE_PROGRAM=pcloud-rs.sh` resolves the package directory with `getcfg`, roots state at `<Install_Path>/var/root`, and delegates to the shared supervisor. Exercise App Center enable/start/stop/restart and service-script status; verify PID ownership, logs, `pcloudc status`, and drain timeout behavior. |
| Upgrade | Upgrade in place from the prior supported QPKG. Prove the chosen volume and `<Install_Path>/var` survive QDK replacement, store migration and vault access succeed, and resumable work continues. `package_routines` is currently empty, so no migration/retention behavior may be inferred from a hook that does not exist. |
| Recovery | Test SIGKILL, stale PID, reboot, storage-pool unavailability/reappearance, and volume migration where supported. The supervisor must refuse to signal an unverified PID, recreate only runtime/log directories, and preserve durable root/journals. Qualify QTS and QuTS hero separately because storage and service behavior differ. |
| Uninstall/reinstall | Record whether App Center removes the QPKG install volume and embedded `var`; export/restore state before uninstall when retention is required. Verify disabled service, no process/socket/mount, and an explicit clean or restored reinstall. Never claim retained state until tested on both firmware families. |
| Hardware evidence | Cover each shipped architecture, representative QTS and QuTS hero models, minimum firmware, install/upgrade/reboot/uninstall, live cloud operations, constrained memory/disk, log growth, and optional FUSE only where vendor policy permits it. |

### ASUSTOR ADM lifecycle

| Stage | Vendor-specific procedure and qualification |
|---|---|
| Build/install | Run the official ADM 5 APKG 2.0 tool through `build-apk.sh` as required by that tool, for `x86-64` and/or `arm64`. Verify the release-approved 90x90 PNG, generated `CONTROL/config.json`, payload ownership after the tool's root-side `chown`, signature/channel policy, and install the exact `.apk` through App Central/manual install. |
| Start/status/stop | ADM invokes `CONTROL/start-stop.sh`, which roots state at `${APKG_PKG_DIR}/var/root` and delegates to the shared supervisor. Exercise App Central start/stop/restart and status, confirm the expected start/stop order, package identity, PID/log paths, `pcloudc status`, and bounded drain. |
| Upgrade | Install the next APKG over the prior candidate after drain. Prove `${APKG_PKG_DIR}/var` retention, transactional store migration, file-vault ownership, and journal resume. The current control metadata has no explicit upgrade or restart-service migration hook, so behavior must be established on ADM hardware. |
| Recovery | Test kill/reboot, stale PID, low disk, package-volume relocation where ADM offers it, and corrupt/missing runtime files while durable state remains. Verify recovery never runs the daemon as root merely to gain FUSE and never discards the state root as a first response. |
| Uninstall/reinstall | Determine and record App Central's treatment of the package-local `var` tree; back it up before uninstall when retention is desired. Verify no service/process/socket/mount remains, then test both clean reinstall and an explicitly restored state. |
| Hardware evidence | Cover the minimum ADM 5 release, every shipped CPU payload, representative hardware, package permission/ownership, install/upgrade/reboot/uninstall, live transfer/share, resource pressure, log rotation, and optional mount only on a model with supported `/dev/fuse` access. |

## Operational controls

| Feature | Why it exists | Good for, and why |
|---|---|---|
| Health endpoint/CLI | Distinguishes a responsive initialized daemon from a mere PID. | Service managers and incident triage. |
| Metrics/SLO/alerts | Exposes rates, failures, latency, queues, breaker and mount/transfer state. | Capacity, fleet health, and release soak. Canonical names prevent dashboard drift. |
| Structured logs | Gives searchable events with redaction and optional JSON. | Support and SIEM. Logs are not an authorization channel. |
| Audit verifier | Detects chain tampering on demand/schedule. | Compliance and snapshot trust. Off-host retention is still required for deletion resilience. |
| Config reload | Applies validated reloadable policy without abrupt process death. | Operations and fleet management. Invalid reload retains the prior state where designed. |
| Graceful drain | Stops new work and waits/reports in-flight completion before shutdown/handoff. | Upgrades and maintenance. Timeout is surfaced rather than falsely claiming clean exit. |
| Orphan mount cleanup | Discovers and removes stale native resources. | Crash/reboot recovery. Native unmount failure remains visible and may need OS tooling. |
| Resource limits | Bounds memory/cache/queues/concurrency/rate/bandwidth. | Small NAS devices through enterprise hosts. Limits make overload behavior deliberate. |
| Capacity planning/benchmarks | Documents/Measures store/cache/transfer/cold-start behavior. | Selecting hardware and safe defaults. Synthetic numbers are not service-wide guarantees. |

## Local CI/CD authority

GitHub Actions workflows are intentionally inactive and archived under
`.github/workflows-disabled/`. The repository-owned Rust `xtask` is the
authoritative pipeline:

| Command | Feature/rationale |
|---|---|
| `cargo xtask preflight` | Verifies tools, pinned workflow posture, and prerequisites before expensive work. |
| `cargo xtask compat` | Proves portable-core MSRV/optional feature combinations separately from the normal 1.96.1 toolchain. |
| `cargo xtask host` | Runs formatting, compile, strict lint, workspace tests, rustdoc/books, audit/deny on the current host. |
| `cargo xtask coverage` | Runs instrumented tests and enforces line coverage strictly above 90%. |
| `cargo xtask package` | Validates NAS, portable Unix, SDK, and package metadata/artifacts. |
| `cargo xtask docker` | Exercises OCI/glibc/musl/Linux portability; it cannot qualify Windows/macOS/BSD kernels. |
| `cargo xtask windows` | Transfers the dirty working tree over SSH and runs native Windows build/test/pipe/DPAPI/optional WinFSP work. |
| `cargo xtask ci` | Runs the complete required local pipeline; skip flags make a run explicitly partial. |
| `cargo xtask release` | Adds reproducible release artifact work after full CI. |

The normal/release workflow toolchain is Rust 1.96.1. Compatibility checks for
older MSRVs are separate contracts, not permission to run the main pipeline
with a different compiler. Docker cannot replace native macOS, BSD, Solaris,
Windows, signing, package-manager, or NAS hardware evidence.

## Release evidence checklist

For each advertised platform/channel, retain:

1. clean release commit and locked dependency graph;
2. native build, tests, lint/docs/security gates;
3. artifact checksum, SBOM, provenance/signature as policy requires;
4. clean install and first start;
5. real account login, upload, download, copy, share, public link, Crypto as
   applicable, plus kernel mount when claimed;
6. restart/crash resume and graceful drain;
7. in-place upgrade preserving state/vault/journals;
8. uninstall and reinstall with explicit retain/remove-data choices;
9. no process, mount, temporary credential, or leaked remote test object;
10. results tied to the exact artifact/commit, not a workflow definition.

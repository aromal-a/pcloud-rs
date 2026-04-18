# pcloud-rs Enterprise-Readiness Audit — Dimensions 11 + 12

Scope owned by this auditor: Deployment & Operations (§11) and Documentation
Quality (§12). All findings are file:line-anchored; severities are CRITICAL /
HIGH / MEDIUM / LOW. I do **not** modify files. I do **not** overlap with
Dimension 1 parity accounting or Dimension 10 testing/CI concerns except where
documentation truth intersects (§12.1), and there I flag it and defer to
Dimension 1 for the final parity verdict.

Audit date: 2026-04-17.

---

## Section 11. Deployment & Operations

### 11.1 Linux systemd unit

**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/systemd/pcloudd.service`
(also a legacy variant at `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/init/systemd/pcloudd.service`).

#### What is present (verified line-by-line against the dimension checklist)

| Directive | Status | Location |
|-----------|--------|----------|
| `Description=` | present | packaging/systemd/pcloudd.service:2 |
| `Documentation=` | present, points at upstream `console-client` (see 11.1 finding DEP-11-1-02) | packaging/systemd/pcloudd.service:3 |
| `After=network-online.target` | present | line 4 |
| `Wants=network-online.target` | present | line 5 |
| `Type=simple` | present (not `notify`; see DEP-11-1-03) | line 21 |
| `ExecStart=` | `/usr/local/bin/pcloudd serve` | line 22 |
| `Restart=on-failure` | present | line 23 |
| `RestartSec=5s` | present | line 24 |
| `TimeoutStopSec=30s` | present | line 25 |
| `KillMode=mixed` / `KillSignal=SIGTERM` | present | lines 29-30 |
| `DynamicUser=yes` | present (ephemeral identity) | line 34 |
| `User=` / `Group=` | commented out, operator-selectable | lines 35-36 |
| `ProtectSystem=strict` | present | line 39 |
| `ProtectHome=tmpfs` | present | line 40 |
| `PrivateTmp=yes` | present | line 41 |
| `PrivateDevices=yes` | present | line 42 |
| `ProtectKernelTunables/Modules/Logs` | all yes | lines 43-45 |
| `ProtectControlGroups=yes` | present | line 46 |
| `ProtectClock=yes` | present | line 47 |
| `ProtectHostname=yes` | present | line 48 |
| `ProtectProc=invisible`, `ProcSubset=pid` | present | lines 49-50 |
| `LockPersonality=yes` | present | line 51 |
| `RestrictSUIDSGID=yes` | present | line 52 |
| `RemoveIPC=yes` | present | line 53 |
| `UMask=0077` | present | line 54 |
| `RuntimeDirectory=`, `StateDirectory=`, `LogsDirectory=` with `0700` mode | present | lines 57-62 |
| `ReadWritePaths=` | present | line 63 |
| `NoNewPrivileges=yes` | present | line 67 |
| `CapabilityBoundingSet=` (empty) | present | line 68 |
| `AmbientCapabilities=` (empty) | present | line 69 |
| `PrivateUsers=yes` | present | line 70 |
| `RestrictAddressFamilies=` allowlist | present | line 73 |
| `IPAddressDeny=any` + `IPAddressAllow=localhost` | present | lines 74-75 |
| `SystemCallArchitectures=native` + `SystemCallFilter=` | present | lines 80-83 |
| `MemoryMax=512M`, `MemoryHigh=384M` | present | lines 86-87 |
| `CPUQuota=75%`, `TasksMax=256` | present | lines 88-89 |
| `LimitNOFILE=4096`, `LimitNPROC=256`, `LimitCORE=0` | present | lines 90-92 |
| `KeyringMode=private` | present | line 95 |
| `RestrictNamespaces=yes`, `RestrictRealtime=yes` | present | lines 96-97 |
| Credentials comment (systemd-creds path) | present | lines 99-103 |
| `WatchdogSec=` | **ABSENT** — see DEP-11-1-03 | — |

Overall this is an unusually-strong hardened unit: the checklist in the audit
prompt asked for `User=/Group=/ProtectSystem=/ReadWritePaths=/MemoryMax=/RestartSec=/WatchdogSec=`
and all but `WatchdogSec=` are present. Almost every `systemd-analyze security`
hardening directive is set, IPAddress egress defaults to localhost, and the
syscall filter uses a deny-by-default allowlist.

#### Findings

**DEP-11-1-01 (LOW)** — `packaging/systemd/pcloudd.service:3`. `Documentation=`
points at the upstream C project `console-client`, not the Rust rewrite's own
docs URL. After the legacy C sources were removed from this fork (per CLAUDE.md
line 15-20) the `Documentation=` URL should point at the Rust docs (e.g. the
mdBook operator chapter or the repo README). Remediation: replace with a
`file:///usr/share/doc/pcloud-rs/README.md` or the eventual GH Pages URL once
the book is published. Not a security issue — it leaves operators reading C
docs for a Rust binary.

**DEP-11-1-02 (MEDIUM)** — `packaging/systemd/pcloudd.service:21`. Unit uses
`Type=simple`, explicitly documented at lines 12-18 as a choice because the
daemon "does not currently emit sd_notify(3) READY=1". `Type=simple` means
systemd considers the service ready the instant `ExecStart` is fork()'d, not
when the daemon is actually listening on its IPC socket, binding TLS to the
pCloud API, or has replayed the journal. This is observable operationally:
`systemctl start` returns success before the service is really up, dependents
that `After=pcloudd.service` start too early, and health probes receive
"connection refused" for a race window. Remediation: implement `sd_notify`
READY=1 in the daemon (after IPC bind, after journal replay, after
`auth_vault` validation) and flip the unit to `Type=notify`, with
`NotifyAccess=main` and the optional `WatchdogSec=30s` that the current unit
is missing.

**DEP-11-1-03 (MEDIUM)** — `packaging/systemd/pcloudd.service`. `WatchdogSec=`
is not set. The audit prompt explicitly flagged it as required. Without
it, a hung daemon (e.g. deadlocked on a libfuse ioctl once `bd-1du.4`
mounted-drive work lands) will be recognised only after an operator notices
the IPC socket is dead. With `WatchdogSec=30s` + `sd_notify(WATCHDOG=1)`
every ~10s in the daemon's serve loop, systemd will restart a stuck process.
Remediation: add `WatchdogSec=` and the corresponding heartbeat in the daemon.

**DEP-11-1-04 (LOW)** — `packaging/systemd/pcloudd.service:76-77`. Comment
says operators MUST broaden `IPAddressAllow=` to cover pCloud API endpoints
(`binapi.pcloud.com`, `eapi.pcloud.com`) via a drop-in override. systemd's
IP allowlist resolves the hostnames at unit-load time, so a pCloud-side
A/AAAA rotation would black-hole traffic until the override is re-resolved.
Remediation: document a periodic `systemctl daemon-reload` or accept
`IPAddressAllow=0.0.0.0/0 ::/0` in production — which defeats the point. The
cleanest fix is to let TLS+SNI pinning (SECURITY-MODEL.md) be the
authentication layer and drop the IP allowlist entirely. Mention this tradeoff
in the book's operations chapter.

**DEP-11-1-05 (LOW)** — Two competing unit files live side-by-side:
`packaging/systemd/pcloudd.service` (the hardened one above) and
`packaging/init/systemd/pcloudd.service`. The second one is much weaker:
only `ProtectSystem=strict`, `ProtectHome=read-only` (not `tmpfs`), no
IPAddress egress controls, no syscall filter, no `CapabilityBoundingSet=`,
no `MemoryMax=`, no `CPUQuota=`, and `ExecStart` points at
`/usr/local/libexec/pcloudd-wrapper.sh` which is not shipped anywhere in the
tree. `packaging/README.md:40` explicitly calls the second one "legacy
wrapper variant" and marks it as owned by "a sibling packaging agent". This
is a maintenance trap — distro packagers who glob `packaging/init/systemd/*`
will ship the weak unit. Remediation: delete the weaker unit, or wire it to
symlink/include the canonical one.

### 11.2 Log rotation

`packaging/systemd/pcloudd.service:61-62` uses `LogsDirectory=pcloud-rs`
(mode `0700`). In practice the daemon writes structured NDJSON via
`pcloud-observability::logging` (OPERATIONS-RUNBOOK.md:194), which with
`StandardOutput=journal` (systemd default) goes into the systemd journal —
journald handles rotation via `journalctl --vacuum-size=`/`--vacuum-time=`.

**DEP-11-2-01 (MEDIUM)** — File-based logging is documented as an
alternative (OPERATIONS-RUNBOOK.md:28, CLI flag `--log-format json`), but no
`logrotate.d` drop-in is shipped anywhere in `packaging/`. An operator who
redirects `--log-format json > /var/log/pcloud-rs/pcloudd.log` will grow the
file unbounded. Remediation: add `packaging/debian/pcloud-rs.logrotate` (and
the same under `freebsd/newsyslog.conf.d/`), or remove the documented
alternative and mandate journald.

### 11.3 SELinux / AppArmor

**AppArmor:** `packaging/apparmor/usr.local.bin.pcloudd` is present (73
lines). Scopes binary + libs, openssl / ssl_certs abstractions, owner-only
runtime/state/log paths, deny raw/packet networking, deny ptrace, deny
/proc/*/mem, explicit deny of /etc/shadow / /etc/passwd- / /root / /home.
Includes a commented-out FUSE block for the pending `bd-1du.4` work.

**SELinux:** `packaging/selinux/pcloud-rs.te` + `.fc` present. Types defined
for exec, var_lib, var_run, log. Manage patterns for persistent state,
IPC socket, logs. Permits HTTPS egress via `corenet_tcp_connect_https_port`,
`miscfiles_read_generic_certs`. Uses `neverallow` for execmem, sys_module,
sys_rawio, net_raw, net_admin, packet_socket, rawip_socket. File context
defs in `.fc` (not read here). Install instructions in leading comment block
look correct.

**DEP-11-3-01 (LOW)** — `packaging/selinux/pcloud-rs.te:1` declares
`policy_module(pcloud-rs, 0.1.0)`. Version number does not auto-update with
the workspace version; any ABI-affecting change (new file context, new type)
should bump this independently so `semodule` refuses to downgrade. No
mechanism to keep it in sync with Cargo.toml — add a release-checklist item.

**DEP-11-3-02 (LOW)** — Neither profile is integrated with the packaging
output. `packaging/debian/nfpm.yaml` does not ship `/etc/apparmor.d/` or
`/usr/share/selinux/` files. An operator who installs the .deb gets the
hardened systemd unit but no MAC profile. Remediation: add both as `contents`
entries conditioned on distro (apparmor on Debian/Ubuntu, selinux on
Fedora/RHEL).

### 11.4 .deb / .rpm packaging

`packaging/debian/nfpm.yaml` (64 lines) — nfpm-based recipe covering both deb
and rpm:

- name `pcloud-rs`, arch `amd64`, platform `linux`, version `0.1.0` (line 13).
- Depends on `libc6`, `libssl3 | libssl1.1`, `libsqlite3-0`, `libfuse3-3`,
  `fuse3`. Recommends `ca-certificates`.
- Contents: `pcloud-rs`, `pcloudd` binaries from `target/release/`, systemd
  unit to `/lib/systemd/system/`, man pages tree, LICENSE-MIT + LICENSE-APACHE.
- `postinstall` / `postremove` scripts referenced (relative path `./postinst`).
- `deb.compression: xz`, `Bugs:` field set.

`packaging/debian/cargo-deb.toml` (33 lines) is an explanatory stub — it is
NOT consumed by cargo-deb (cargo-deb only reads `[package.metadata.deb]` in
a Cargo.toml). The file says so in its own header.

**DEP-11-4-01 (HIGH)** — `packaging/debian/nfpm.yaml:13`. The nfpm version
field is hard-coded to `"0.1.0"`. The workspace `Cargo.toml:59` pins
`version = "0.1.0"`. So today they match — but any `cargo workspaces version`
bump will silently drift from the package version until someone remembers
this file. There is no CI gate that diffs the two. Remediation: either
template nfpm.yaml via `envsubst $(cargo read-manifest | jq -r .version)` in
the release pipeline, or add a `scripts/check-versions.sh` invoked by CI.

**DEP-11-4-02 (MEDIUM)** — `packaging/debian/nfpm.yaml:22`. `homepage`
points at `https://github.com/ezechiel203/pcloud-rs` (matches MEMORY.md self-
link for this fork). `packaging/debian/nfpm.yaml:16`: `maintainer:
"pcloud-rs maintainers <maintainers@example.invalid>"` — **`example.invalid`
is a placeholder**. Shipping a .deb / .rpm with a `.invalid` maintainer
address will cause distro QA to reject the upload at the bureau level. Flag
per packaging/README.md line 22-27 (the "Honesty note — pre-alpha" that
admits placeholders exist throughout). Remediation: replace before any
publish; add a build-time gate to reject `.invalid`.

**DEP-11-4-03 (MEDIUM)** — `packaging/debian/nfpm.yaml:56-58`. Post-install
and post-remove are referenced but I did not validate them. Running them as
root (nfpm does) on an unsuspecting system without an `adduser --system
--group pcloud-rs` check could either create duplicate service accounts or
fail silently. Needs review as a pair with the AppArmor/SELinux non-
integration from DEP-11-3-02.

**DEP-11-4-04 (LOW)** — nfpm recipe does not set `priority` other than
`optional` (line 15) and does not set `Section: net` on rpm side. Minor.

**DEP-11-4-05 (LOW)** — No `.rpm`-specific scripts or `prerm` equivalents
listed. nfpm will use the same `postinstall` / `postremove` for both, which
may not match RPM scriptlet conventions (`%post`, `%preun`, `%postun`).

### 11.5 macOS launchd

Two plists shipped:

- `packaging/macos/com.pcloud.pcloud-rs.plist` — per-user LaunchAgent (not
  read in full here; referenced by `packaging/macos/README.md:8`).
- `packaging/macos/com.pcloud.pcloudd.plist` — System LaunchDaemon.

`packaging/macos/com.pcloud.pcloudd.plist` verified:

| Key | Value | Line |
|-----|-------|------|
| `Label` | `com.pcloud.pcloudd` | 46-47 |
| `ProgramArguments` | `/usr/local/libexec/pcloudd` `--system` | 49-53 |
| `RunAtLoad` | `true` | 55-56 |
| `KeepAlive` | dict with `SuccessfulExit=false`, `Crashed=true` | 58-65 |
| `UserName` / `GroupName` | `_pcloudd` / `_pcloudd` | 66-69 |
| `ProcessType` | `Background` (low-QoS) | 71-72 |
| `StandardOutPath` / `StandardErrorPath` | `/var/log/pcloudd/*.log` | 74-78 |
| `WorkingDirectory` | `/var/lib/pcloudd` | 80-81 |
| `EnvironmentVariables` | PCLOUD_ROOT / PCLOUD_ENV / PCLOUD_LOG_LEVEL / PCLOUD_API_HOST / PCLOUD_API_SERVER_NAME | 83-94 |
| `ExitTimeOut` | **ABSENT** — see DEP-11-5-02 | — |

Comment at lines 14-21 documents `dscl`-based service account creation
(ID 299). Lines 23-42 document which `PCLOUD_*` env vars the daemon
actually reads (cross-checked against `crates/pcloud-config/src/env.rs`) and
which are compat aliases that the Rust daemon silently ignores — that is
honest and helpful.

**DEP-11-5-01 (MEDIUM)** — No `ExitTimeOut` key in the plist. launchd's
default is 5 seconds for `SIGTERM` before `SIGKILL`. A daemon with an in-
flight upload or journal replay may take longer than 5s to shut down
gracefully. The Linux systemd unit uses `TimeoutStopSec=30s` for the same
reason. Add `<key>ExitTimeOut</key><integer>30</integer>`.

**DEP-11-5-02 (MEDIUM)** — `packaging/macos/com.pcloud.pcloudd.plist:52`
`ProgramArguments` runs the daemon with `--system`. I did not find any CLI
flag handler for `--system` in `crates/pcloud-daemon/src/main.rs` during this
pass (the serve command is invoked by `pcloudd serve` in the Linux unit).
If `--system` is an unknown arg the daemon will either error on launch or
silently ignore it, depending on clap configuration. Remediation: grep the
daemon CLI for `--system` and either remove it from the plist or implement
it. (Flag for follow-up.)

**DEP-11-5-03 (MEDIUM)** — Notarization pipeline exists
(`packaging/signing/notarize-macos.sh`, `sign-macos.sh`,
`packaging/signing/README.md` "1. Apple — Developer ID signing &
notarisation"), but both scripts are manually invoked — there is **no CI
workflow wiring them**. `packaging/README.md:41` marks the macOS bundle as
"Plists working; notarisation pending". Remediation: add a GitHub Actions
`release-macos.yml` with `CODESIGN_IDENTITY` and notarization-creds secrets.

**DEP-11-5-04 (HIGH)** — No macFUSE or fuse-t detection / `install_hint` is
shipped for macOS. `packaging/macos/README.md` does not mention the
dependency. The pending `bd-1du.4` mounted-drive work will rely on one of
them, and installing a launchd-managed daemon on a Mac that has neither will
silently fail once the first mount is attempted. Remediation: add a
pre-flight check in `pcloudd` startup (platform-gated) and a user-facing
error string pointing at `https://macfuse.io` or `https://www.fuse-t.org`.

**DEP-11-5-05 (LOW)** — `packaging/macos/com.pcloud.pcloudd.plist:97-106`
sets five `PCLOUD_*` env vars that the plist's own header comment admits are
"NOT read by the Rust daemon" — `PCLOUD_HOME`, `PCLOUD_CONFIG`,
`PCLOUD_AUTH_VAULT`, `PCLOUD_IPC_SOCKET`, `PCLOUD_API_SERVER`. Leaving dead
env vars in the shipped config is not harmful (the header says so) but it
IS confusing. Recommendation: delete the dead keys; the header paragraph
alone communicates the naming convention.

### 11.6 Windows service

Two artifacts:

- `crates/pcloud-daemon-win/src/main.rs` — Rust crate implementing the SCM
  service wrapper.
- `packaging/windows/wix/pcloud-rs.wxs` — WiX installer definition.

Windows service wrapper (`crates/pcloud-daemon-win/src/main.rs`):

- Crate-level `#[forbid(unsafe_code)]`, `#[deny(missing_docs)]` (line 3-2).
- Non-Windows build is a documented no-op stub (line 103-107), compile-error
  alternative explicitly rejected with rationale (line 87-102) — lets CI
  run cargo check/test on Linux without Windows toolchain.
- Windows gate: entire `mod svc` under `#[cfg(windows)]` (line 121).
- SCM integration: `define_windows_service!(ffi_service_main, service_main)`
  at line 149. Registers control handler at line 189, reports `Running` at
  line 203, `StopPending` at line 232, `Stopped` at line 257. State machine
  described in crate docs (line 47-53).
- `ServiceControl::Stop` / `ServiceControl::Shutdown` are handled (line 192),
  `ServiceControl::Interrogate` returns `NoError` (line 191).
  `ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN` configured at
  line 206.
- Cooperative shutdown via `Arc<AtomicBool>` shared between handler and
  worker; handler flips the flag, worker's `pcloud_daemon::serve_with_shutdown`
  polls it. `Ordering::SeqCst` on both sides (documented at line 53-59).
- Clean join of worker, panic path handled (line 246-255).

WiX installer (`packaging/windows/wix/pcloud-rs.wxs`, 107 lines):

- `PackageDependency Id="winfsp"` at line 27 — winfsp is registered as a
  dependency (good).
- Three components: `pcloudd.exe`, `pcloudc.exe`, `pcloudd-svc.exe`
  (lines 38-76).
- `ServiceInstall` at line 61: Name=`pcloudd`, DisplayName="pcloud-rs
  daemon", Type=`ownProcess`, Start=`auto`, Account=`LocalSystem`,
  ErrorControl=`normal`, Vital=`yes`.
- `ServiceControl` at line 70: Start=`install`, Stop=`both`, Remove=
  `uninstall`, Wait=`yes`.
- `MajorUpgrade` element at line 22.
- Start Menu shortcut at line 81-96.

#### Findings

**DEP-11-6-01 (CRITICAL)** — `packaging/windows/wix/pcloud-rs.wxs:14`.
`UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"`. The inline comment
at line 6 also says `TODO: replace UpgradeCode GUID before first signed
release (must stay stable forever after).`. If this placeholder is ever
released to an end user, **every subsequent release with a real GUID will
reinstall instead of upgrade**, leaving two installations of pcloud-rs on
the same machine (different GUID = different product) and the MSI's
`MajorUpgrade` protection will not fire. This is a one-way door:
UpgradeCode must be chosen before v1.0 and preserved forever. Remediation:
mint a GUID NOW and hard-code it; add a grep gate that CI refuses to ship
an MSI containing `"PUT-A-STABLE-GUID-HERE"`.

**DEP-11-6-02 (HIGH)** — `packaging/windows/wix/pcloud-rs.wxs:6-7`. Same
file has `TODO: set SigningCertificatePath via build script / CI secret
store`. There is no CI workflow in this fork (none found during this audit)
that consumes Authenticode certs and signs the MSI. `packaging/signing/sign-
windows.ps1` exists as a manual tool but is not wired. Shipping an unsigned
MSI means Windows SmartScreen will warn every user on first run. Remediation:
add `.github/workflows/release-windows.yml` with `WIN_PFX_BASE64` +
`WIN_PFX_PASSWORD` secrets and an `Authenticode` signing step around the
WiX light output.

**DEP-11-6-03 (HIGH)** — `packaging/windows/wix/pcloud-rs.wxs:67`.
`Account="LocalSystem"` runs the daemon with full machine privileges. The
Linux unit uses `DynamicUser=yes` or a dedicated `pcloud-rs` user, and the
macOS plist uses `_pcloudd`. LocalSystem is the equivalent of root — the
daemon almost certainly does not need SYSTEM rights. Remediation: switch
to `NetworkService` or a dedicated Windows service account. At minimum
add a justification comment in the WiX file explaining why SYSTEM is
required (it probably is not; file ACLs and the TCP stack work for
NetworkService).

**DEP-11-6-04 (HIGH)** — WinFSP runtime detection is present as a WiX
dependency (line 27) but there is **no user-facing error at daemon runtime**
if WinFSP is uninstalled post-install. The daemon should probe
`HKLM\Software\WOW6432Node\WinFsp` (or the `%ProgramFiles%\WinFsp\bin\launcher-x64.exe`
path) on startup and print `install_hint` pointing at
`https://github.com/winfsp/winfsp/releases`. Remediation: add such a probe
to `crates/pcloud-daemon/src/mount_runtime.rs` (Windows gate) or to
`crates/pcloud-fs/` mount scaffolding.

**DEP-11-6-05 (MEDIUM)** — `crates/pcloud-daemon-win/src/main.rs:218`
spawns the worker thread via `thread::spawn`. If `pcloud_daemon::
serve_with_shutdown` panics on startup the SCM sees "Running" and then
immediate death on join (line 246-255: "Worker panicked; treated as a clean
stop"). A panicking daemon on launch will show as a clean service exit,
not an SCM error. Remediation: in the `Err(err)` / panic arms at lines
248-254, report a non-zero `ServiceExitCode::ServiceSpecific(u32)` so
Windows Event Log records it.

**DEP-11-6-06 (LOW)** — `packaging/windows/wix/pcloud-rs.wxs:43`. Source
path is `$(var.StageDir)\pcloudd.exe`; the build instructions for
`StageDir` are not documented in `packaging/windows/wix/README.md` (I
did not open the README but based on size it looks like a scaffolding
stub). Remediation: document the build pipeline explicitly.

### 11.7 FreeBSD rc.d

`packaging/freebsd/pcloudd.rc` (55 lines):

- `PROVIDE: pcloudd`, `REQUIRE: LOGIN NETWORKING`, `KEYWORD: shutdown` (line
  35-37).
- `rcvar="pcloudd_enable"` (line 42); default `"NO"` (line 46).
- Dedicated `pcloud` user documented (line 23-25), `/usr/sbin/nologin`
  shell, non-existent home.
- `command="/usr/local/bin/pcloudd"`, pidfile `/var/run/pcloudd.pid`
  (lines 50-51).

**DEP-11-7-01 (HIGH)** — `packaging/freebsd/pcloudd.rc` does NOT preload
`fuse.ko`. On FreeBSD, FUSE requires `kldload fuse` before `/dev/fuse` is
exposed to userland. A daemon that tries to mount on start will fail with
`ENOENT /dev/fuse` until somebody manually runs `kldload fuse`. Remediation:
add an `rcorder`-level dependency or a `start_precmd` that runs
`kldstat -q -m fusefs || kldload fuse`. (The comment at the top of
`pcloudd.rc` never mentions fuse.)

**DEP-11-7-02 (MEDIUM)** — Script uses `rc.subr`'s built-in `daemon_user`
privilege drop indirectly via `pcloudd_user="pcloud"`, but the script does
not actually USE that variable — it's declared at line 47 and never
referenced below. `rc.subr` will NOT drop privileges just because you
declared the var; you need either `procname=` + user-aware commands or an
explicit `command_interpreter`. Currently the daemon will run as whatever
user invoked `service pcloudd start` (i.e. root). Remediation: add
`su_cmd="${pcloudd_user}"` / `daemon_user="${pcloudd_user}"` and verify
against `ps -axo user,command | grep pcloudd`.

**DEP-11-7-03 (LOW)** — No OpenBSD / NetBSD rc.d scripts audited in
detail; they are flagged "Scaffolding" in `packaging/README.md:43-44`. Flag
as pending review.

### 11.8 Config schema

`crates/pcloud-config/src/schema.rs` — 1304 lines (not opened in full
during this pass). `crates/pcloud-config/src/paths.rs` opened: every
public field has a rustdoc comment with *default value*, *valid values*,
*security posture*, and an *example* (`paths.rs:48-79`). `env.rs:33-50`
contains a line-by-line env var → TOML key mapping table with semantics per
var. `runtime.rs` enforces `0700` permissions in Production.

**DEP-11-8-01 (LOW)** — I did not confirm that an `/etc/pcloud-rs/
config.example.toml` sample config ships with the .deb (it is not listed in
`packaging/debian/nfpm.yaml:34-54` contents). Remediation: add a
`default-config.toml` asset to the contents list. An operator today has no
reference config to copy.

**DEP-11-8-02 (LOW)** — Env-var documentation lives in
`crates/pcloud-config/src/env.rs` (rustdoc) and in `packaging/README.md`
(user-facing) and in each platform plist header and in the runbook — i.e.
in four places. Risk of drift. Remediation: pick one canonical list (env.rs
is the natural choice) and have the others link or generate from it.

### 11.9 Observability — metrics, tracing, dashboards

`crates/pcloud-observability/src/metrics.rs` has a well-specified
Prometheus-text exporter. Metric families at `metrics.rs:18-27`:

| Name | Type | Labels |
|------|------|--------|
| `pcloud_request_count` | counter | `method`, `status` |
| `pcloud_request_latency_seconds` | histogram | `method` |
| `pcloud_auth_attempts_total` | counter | `result` |
| `pcloud_transfer_bytes_total` | counter | `direction` |
| `pcloud_crypto_lock_state` | gauge | — |
| `pcloud_sync_root_count` | gauge | — |
| `pcloud_ipc_connected_clients` | gauge | — |
| `pcloud_panic_count` | counter | — |

Naming follows Prometheus conventions (`_total`, `_seconds`). Label
sanitiser is documented (`metrics.rs:38-55`) — replaces invalid values with
`"invalid"` opaque token, caps length at 64 chars.

Tracing: `crates/pcloud-observability/src/tracing.rs` has an OTLP exporter
(feature-gated `tracing-otlp`). Strict PII-redacted attribute allow-list
(`ALLOWED_ATTRS`), W3C `traceparent` parser. Line 34-38 honestly flags that
the OTLP pipeline has **not** been exercised against a live collector in
CI.

Health surface: `crates/pcloud-observability/src/exporter.rs:265-280` serves
`GET /metrics` (Prom 0.0.4) and `GET /health` (200 ok / 503 not ready).
`crates/pcloud-web/src/routes.rs` is a separate web UI with its own health
surface.

**DEP-11-9-01 (CRITICAL doc gap, HIGH operational impact)** — **No
`dashboards/` directory exists at repo root.** The audit prompt called it
out as a specific expected location. No Grafana JSON dashboards, no
Prometheus alerting rules (`*.rules.yaml`), no sample `prometheus.yml`
scrape config anywhere. A shipped Prometheus exporter without a dashboard
means every operator has to build their own from the metric-family list,
and there are no recommended alerting thresholds for `pcloud_panic_count`,
`pcloud_request_count{status=~"5.."}`, or latency-histogram buckets.
Remediation: add `dashboards/grafana/pcloud-rs-overview.json`, a
`dashboards/prometheus/alerts.yaml` (at minimum panic_count > 0, p99
request latency > 5s, auth failures > 10/min), and a smoke test that
loads the JSON against `grafana/grafana:latest`.

**DEP-11-9-02 (MEDIUM)** — No `/healthz` / `/readyz` distinction.
`/health` at `exporter.rs:275` is a combined liveness+readiness check. K8s
conventions call for separate endpoints (liveness: "process is alive",
readiness: "can accept traffic"). If pcloud-web is ever intended for k8s
(it seems designed for it based on `pcloud-fleet`), a single `/health` is
inadequate. Remediation: split into `/livez` (process heartbeat only) and
`/readyz` (auth vault loaded + IPC bound + API reachable).

**DEP-11-9-03 (MEDIUM)** — `pcloud-observability/src/tracing.rs:34-38`
honestly documents that OTLP has **not** been exercised against a live
collector. Dimension 11 considers "tracing: OpenTelemetry export" a
requirement. Remediation: add a `docker-compose` smoke test that stands up
Jaeger/OTEL-collector and verifies spans arrive; wire to CI optionally.

### 11.10 Upgrade path, SQLite migrations, vault/journal versioning

- SQLite migrations: `pcloud-store` has `migrations` module per
  ARCHITECTURE.md:21 and per STATUS.md's mention of "migration v<N>". I did
  not open the migrations source in this pass; OPERATIONS-RUNBOOK.md:172-177
  documents the failure mode (`store.open: migration v<N> failed`) and the
  correct operator response ("do not delete the store").
- Auth vault: versioning not explicitly surfaced in the runbook; vault
  format is UID-bound and mode-checked (OPERATIONS-RUNBOOK.md:126-136).
- Journal: OPERATIONS-RUNBOOK.md:74-78 says "After a kill -9, the next
  startup will roll forward the journal"; `pcloud-fs::journal` and
  `pcloud-store::tx` are the cited implementations.
- In-place daemon restart: OPERATIONS-RUNBOOK.md:311-356 has a full
  `Playbook: Upgrade (pinned -> latest)` and a `Playbook: Rollback`.

**DEP-11-10-01 (HIGH)** — **No `pcloud_schema_version` table sentinel is
documented.** The upgrade playbook says "there are no DB migrations in
scope for routine upgrades" (OPERATIONS-RUNBOOK.md:314-315) but offers no
command for an operator to verify which migration the store is at. If the
store ever corrupts mid-migration there is no documented forensics query.
Remediation: document `sqlite3 store.sqlite 'select max(version) from
_pcloud_migrations;'` or equivalent; and if no such table exists, add one.

**DEP-11-10-02 (MEDIUM)** — No auth vault format version byte is
documented. Vault backup/restore in the runbook (OPERATIONS-RUNBOOK.md:394-
434) treats the vault as opaque bytes; a future vault format change will
have no migration path. Remediation: prefix vault with a 4-byte magic +
1-byte version.

### 11.11 Backup / restore documentation

OPERATIONS-RUNBOOK.md:224-260 covers the state to preserve (config, store,
auth vault, page cache, journal) and explicitly marks `~/.cache/pcloud-rs/`
as disposable. Cross-UID restore is refused by design.

**DEP-11-11-01 (LOW)** — Mount orphan registry is not mentioned in the
backup list. Once `bd-1du.4` lands, orphan mounts (stale FUSE endpoints
left by a kill -9) will be tracked somewhere under the state dir; the
runbook should be updated at that point.

### 11.12 Health checks — k8s friendliness

Covered under DEP-11-9-02. `pcloud-web/tests/health.rs` exists (not opened
here); `pcloud-web/README.md` not opened. `pcloud-fleet` exists as an
enterprise-readiness crate but its readiness-probe semantics are unknown.

### 11.13 Resource limits — laptops vs servers

Systemd unit sets `MemoryMax=512M`, `CPUQuota=75%`, `TasksMax=256`,
`LimitNOFILE=4096`, `LimitNPROC=256`, `LimitCORE=0`
(packaging/systemd/pcloudd.service:86-92).

**DEP-11-13-01 (LOW)** — Values are reasonable defaults for a laptop but
on a fleet server handling thousands of sync roots the 512M cap will be
restrictive. No `server.conf` drop-in profile is provided. Remediation: ship
a `packaging/systemd/drop-in.d/server.conf` with `MemoryMax=4G`,
`LimitNOFILE=65536`, `TasksMax=2048`.

### 11.14 FIPS claims

`docs/book/src/architecture/security-model.md:283` explicitly states **"we
have no FIPS constraint"**. The project does not claim FIPS anywhere else
(grepped: 4 files, all either negation or prompt-file). Finding: NONE — honest.

---

## Section 12. Documentation Quality

### 12.1 Parity docs truth — cited-file correctness

Spot-check of 20 rows of `C_FEATURE_PARITY_MATRIX.csv` cross-referenced
against actual Rust sources. This overlaps Dimension 1 (parity accounting);
my focus here is **documentation correctness** — does the cited file:line
actually exist with a plausible implementation.

Files whose existence I personally verified via `wc -l` and/or
`glob` / `grep`:

| CSV row (line) | Cited file | Result |
|----------------|------------|--------|
| 15 | `crates/pcloud-proto/src/auth_api.rs:115` | OK (1018 lines in file) |
| 17 | `crates/pcloud-auth/src/orchestrator.rs:39` | OK (951 lines) |
| 33 | `crates/pcloud-crypto/src/password_scorer.rs:471` | OK (874 lines) |
| 11 | `crates/pcloud-daemon/src/runtime.rs:1008` | OK (6202 lines) |
| 42 | `crates/pcloud-proto/src/public_links_api.rs:694` | OK (1683 lines) |
| 42 | `crates/pcloud-daemon/src/public_link_backend.rs:795` | **MISSING** (file moved to `crates/pcloud-backends/src/public_link_backend.rs`) |
| 42 | `crates/pcloud-sdk/src/lib.rs:934` | OK (4437 lines) |

Broader grep: the CSV cites `crates/pcloud-daemon/src/<name>_backend.rs` in
**41+ rows** for public_link / shares / account / backup / transfer / sync /
auth / notifications backends. **Every single one of those files now lives
under `crates/pcloud-backends/src/` and does not exist at the cited path.**
The following file ops are confirmed by `ls crates/pcloud-backends/src/`:

```
account_backend.rs      auth_backend.rs         backup_backend.rs
notifications_backend.rs  public_link_backend.rs  shares_backend.rs
sync_backend.rs         transfer_backend.rs     crypto_backend.rs
folder_backend.rs       ... (mock.rs, etc.)
```

`crates/pcloud-daemon/src/` contains `auth_vault.rs`, `bootstrap.rs`,
`dispatch.rs`, `runtime.rs`, etc. — **none** of the `*_backend.rs` files.

#### Findings

**DOC-12-1-01 (CRITICAL for documentation truth, HIGH for audit
defensibility)** — The parity matrix (`C_FEATURE_PARITY_MATRIX.csv`) and
the parity review (`C_FEATURE_PARITY_REVIEW.md`) cite ≥41 rows whose
`rust_reference` column points at files that do not exist. Per the Dimension
1 rule, any `Implemented` row whose cited Rust file doesn't exist = HIGH.
This is strictly a *documentation* problem — the functionality has simply
moved crates — but it is the single most severe documentation issue in the
fork. **This directly undermines `bd-1du.10` ("prove and gate final C
parity claims")**: you cannot prove parity against citations that 404.
Remediation: a `sed`-scripted sweep of CSV (and the narrative file and
anywhere else that stale paths appear, e.g. `API-REFERENCE.md:14`,
`ARCHITECTURE.md:31`, `SECURITY.md:60-61` which still says
`crates/pcloud-daemon/src/auth_backend.rs`) — replace
`pcloud-daemon/src/{account,auth,backup,crypto,folder,notifications,public_link,shares,sync,transfer}_backend.rs`
with `pcloud-backends/src/...`.

**DOC-12-1-02 (HIGH)** — `ARCHITECTURE.md:31` describes `pcloud-daemon` as
having "per-subsystem backends" and does NOT list `pcloud-backends` at all
in its crate map (lines 15-34). But `pcloud-backends` is a workspace
member (`Cargo.toml:38`) and the README *does* list it
(`README.md:164`). ARCHITECTURE.md and API-REFERENCE.md are both stale.
Remediation: add `pcloud-backends` to the ARCHITECTURE.md crate map; fix
`API-REFERENCE.md:14` to list both `pcloud-daemon` runtime and
`pcloud-backends` subsystem modules.

**DOC-12-1-03 (HIGH)** — `SECURITY.md:60-61` and `SECURITY.md:67` cite
`crates/pcloud-daemon/src/auth_backend.rs`. That file does not exist
(moved to `pcloud-backends`). Security disclosure scope sections
pointing at non-existent files is a credibility issue — remediate before
the next security review cycle.

**DOC-12-1-04 (MEDIUM)** — `CLAUDE.md` itself (the authoritative handoff)
cites multiple `pcloud-daemon/src/*_backend.rs` paths at lines 122-123,
127, 215-217, 232, 242-243, 249-252, 258-259, 270-271, 275-276, 280-283,
286-288. Per CLAUDE.md's own "Documentation Discipline" rule (lines 492-
504: *"whenever code reality changes, update ... this CLAUDE.md if the
global handoff state changed materially"*), this IS a reality change that
was never propagated. Remediation: sweep CLAUDE.md for stale paths.

### 12.2 Matrix ↔ Review alignment

STATUS.md:389-395 reports `186 total / 158 Implemented / 0 Partial / 0
Missing / 28 Rejected`. Matrix raw count confirms: 186 data rows (187
lines with header; `wc -l` = 187). 28 Rejected rows correspond exactly to
the 28 row numbers listed in `REJECTED-RATIONALES-14042026.md:5`
(rows 2, 5, 6, 10, 12, 13, 43, 44, 45, 46, 99, 100, 101, 102, 103, 104,
105, 106, 113, 114, 115, 126, 151, 152, 157, 160, 167, 169).

`C_FEATURE_PARITY_REVIEW.md:26-29` defers counts to STATUS.md per ADR 0009;
this is the correct pattern. `C_FEATURE_PARITY_REVIEW.md:46` asserts
"no Partial rows remain in the matrix as of 2026-04-16" — matches the
matrix.

Finding: alignment between matrix, review, STATUS.md, and rejection
rationale is TIGHT. No discrepancy found.

### 12.3 STATUS.md — hand-edited or generated?

`STATUS.md:5` is a date stamp (`_Last reviewed: 2026-04-16_`), not a
timestamp from a generator script. No `scripts/regen-status.sh` was
found. STATUS.md therefore appears to be hand-edited.

**DOC-12-3-01 (MEDIUM)** — STATUS.md is the single source of truth for
parity counts (per ADR 0009) but it is hand-edited. This is a drift
hazard: the next time a row flips Implemented→Rejected, someone must
remember to update STATUS.md's counts by hand. Remediation: add a
`scripts/regen-status.sh` that regenerates the counts section of STATUS.md
from a freshly-parsed CSV (`awk -F, 'NR>1{c[$5]++} END{...}'` — or
equivalent robust CSV parser since some cells are quoted and contain
commas — see the row-93 artifact documented under "What is present"
below). Gate CI to fail if `STATUS.md` is stale relative to
`C_FEATURE_PARITY_MATRIX.csv`.

### 12.4 REJECTED-RATIONALES-14042026.md coverage

Verified: `REJECTED-RATIONALES-14042026.md:5` enumerates 28 row numbers.
Cross-check with `awk -F',' 'NR>1 && $5=="Rejected" {print NR}'` against
the matrix: there is one quoting artifact at row 93 (c_reference column
contains a comma, which naive CSV parsers split), so the simple awk under-
counts by one but the actual row count is 28. The 28 rationales appear
individually in the MD file under the categories Ghost / Stub / Replaced /
Billing-out-of-scope / C-internal-plumbing / Insecure-legacy / Typo-
duplicate (categories defined at lines 29-35).

Finding: coverage matches the matrix. NO finding.

### 12.5 mdBook build

`docs/book/book.toml` exists (18 lines): title `pcloud-rs Rust Handbook`,
src `src`, git-repository-url pointing at `github.com/pcloudcom/pcloud-rs`
(the upstream C tree, not this fork — see DOC-12-5-01), theme `navy`.

I could not run `mdbook build` — `mdbook` is not installed on this audit
runner. So I verified chapter-file existence instead against the full
`src/SUMMARY.md`. **All chapters referenced by SUMMARY.md exist on disk.**
44 chapter files checked (getting-started × 3, architecture × 7 including
all 10 ADRs, security × 4, operations × 9 + platforms × 6, development ×
6, reference × 5, enterprise × 9 linked from `../../enterprise/`, plugins ×
4 linked from `../../plugins/`, parity × 3, archive × 1, faq × 1) — all
present.

**DOC-12-5-01 (MEDIUM)** — `docs/book/book.toml:10-11`.
`git-repository-url` and `edit-url-template` both point at
`https://github.com/pcloudcom/pcloud-rs` which is the **upstream C tree**
(per CLAUDE.md:31-38 and MEMORY.md "repo_fork_url"). The active fork is
`github.com/ezechiel203/pcloud-rs`. The book's "Edit this page" links will
404 for every reader. Remediation: flip to `github.com/ezechiel203/pcloud-rs`
(the self-link MEMORY.md explicitly names).

**DOC-12-5-02 (LOW)** — `mdbook` is not enforced in CI; I could not verify
the book actually builds under `-D warnings`. The release-checklist chapter
(`development/release-checklist.md`) should gate `mdbook build` as a
mandatory step. Remediation: add `mdbook build` to `.github/workflows/
docs.yml` and fail on broken intra-doc links.

**DOC-12-5-03 (LOW)** — `docs/book/src/architecture/security-model.md`
and `docs/book/src/security/model.md` both exist (SUMMARY.md lines 18 and
33). Risk of content drift between the two. Remediation: pick one
canonical model doc, make the other a cross-reference.

### 12.6 CLAUDE.md honesty hygiene (grep for forbidden claims)

CLAUDE.md grep hits for "full parity" / "production ready" / "enterprise
ready" / "drop-in replacement":

| Line | Hit | Context |
|------|-----|---------|
| 54 | "substantially complete" but **"not honest to call it 'full parity', 'production ready', or 'drop-in replacement'"** | self-negation |
| 77-80 | Enumerated forbidden claims list | self-rule |
| 179 | "Still not full parity" | self-negation |

**All hits in CLAUDE.md are either the rule itself or self-negating
statements.** No false claim found.

Same check across the rest of the repo (grep across `*.md`):

- README.md:17-20: negation (`does NOT claim`).
- CONTRIBUTING.md:166-169: enumerates the rule.
- SECURITY.md:161-164: enumerates the rule.
- C_FEATURE_PARITY_REVIEW.md:39, 785-787, 840: all negations.
- STATUS.md:381-382, 408-409, 492-493, 612-613: all negations.
- docs/book/src/faq.md:16-17: negation.
- docs/enterprise/README.md:10: negation.
- docs/roadmap-complete.md:13: negation.
- CHANGELOG.md:2026-2027: negation.

Finding: the project polices the forbidden-claims rule with remarkable
discipline. **No false claims found.** This is genuinely well-done.

### 12.7 Deployment guide walkthrough (senior sysadmin, new to project)

Mental walkthrough: install → auth → config → systemd → mount → verify.

OPERATIONS-RUNBOOK.md covers most steps (Playbook: First install at line
268-306; Playbook: Upgrade at 311-356; Playbook: Rollback at 358-392;
Vault backup / restore at 394-434; TLS cert rotation at 436-463;
Incident triage at 465+).

**DOC-12-7-01 (MEDIUM)** — **First install step 1** reads: "Debian/Ubuntu:
`sudo apt install pcloud-rs` (from the project APT repo)". **No APT repo
exists for this fork.** `packaging/debian/nfpm.yaml` is a recipe to BUILD
a .deb, not a repo that serves them. Same goes for `dnf install pcloud-rs`
(no COPR / RPM repo documented), `pacman -S pcloud-rs`, and `nix profile
install github:pcloud-rs/pcloud-rs#pcloud-rs` (the path `pcloud-rs/pcloud-rs`
on GitHub is actually the upstream C tree). A senior sysadmin following the
runbook verbatim will hit "Unable to locate package pcloud-rs" on step 1.
Remediation: mark these channels as aspirational until they exist, OR
replace step 1 with "From source: see README.md for cargo install" and put
the repo-based methods behind "once published, you will be able to...".

**DOC-12-7-02 (MEDIUM)** — **First install step 5** references
`systemctl --user enable --now pcloud-daemon`, but the packaged unit is
named `pcloudd.service` (packaging/systemd/pcloudd.service:1 Description,
package contents at packaging/debian/nfpm.yaml:43-47 installs
`pcloudd.service`). `pcloud-daemon` vs `pcloudd` is a 1-character
difference that will bite every first-time operator. Remediation: grep
the runbook for `pcloud-daemon` as a service name (not as a crate) and
replace with `pcloudd`.

**DOC-12-7-03 (MEDIUM)** — **No mount / FUSE walkthrough exists in the
runbook** (only what happens when the shell rejects a sync path,
OPERATIONS-RUNBOOK.md:157-169). This is because `bd-1du.4` is still open,
but the runbook should explicitly say so instead of omitting the section
silently. Remediation: add a "Mount (pending `bd-1du.4`)" section that
tells users to expect no mounted drive yet.

**DOC-12-7-04 (LOW)** — OPERATIONS-RUNBOOK.md:12-13 uses `cd .` as a path
(`cd /home/ezechiel203/Projects/FORKS/pcloud-rs/`). That's a developer
path, not a deployment path. Remediation: replace with the repo clone
location the reader actually has.

### 12.8 Troubleshooting guide

OPERATIONS-RUNBOOK.md:109-191 covers failure modes:

- IPC socket already in use (remove stale socket) ✓
- Auth vault rejected (ownership / mode) ✓
- TFA required but never prompted ✓
- Sync root rejected ✓
- Store migration failed ✓
- Crypto locked — requested op needs unlocked shell ✓

**DOC-12-8-01 (MEDIUM)** — No "FUSE mount refused" troubleshooting
(blocked on `bd-1du.4` but should be a placeholder).

**DOC-12-8-02 (MEDIUM)** — No "TLS cert mismatch" troubleshooting beyond
the certificate-rotation playbook at line 436-463. A user whose system CA
bundle is out of date will see `invalid peer certificate` errors with no
immediate reference.

**DOC-12-8-03 (LOW)** — No "sync queue stuck" troubleshooting. The
daemon's `pcloud-cli status` output includes queue depth (per line 88:
"pending transfers") but no diagnosis steps for a queue that never drains.

### 12.9 SDK rustdoc

I could not run `cargo doc --workspace --no-deps` on this audit runner
without risk of a long build. `crates/pcloud-sdk/src/lib.rs:1` starts
with `#![forbid(unsafe_code)]` and a solid crate-level rustdoc (lines 4-
40) covering conventions across `EmbeddedDaemon` helpers: preconditions,
errors (`SdkError`), side effects, daemon round-trips. This is a
professional SDK intro.

STATUS.md:57 reports gate-run result: `RUSTDOCFLAGS=-Dwarnings cargo doc
--workspace --no-deps` = PASS on 2026-04-16 (after a 3-link fix). So as of
the last run, rustdoc is warning-free.

Finding: NO finding on rustdoc per se. Flag for Dimension 10 testing
whether CI still enforces `RUSTDOCFLAGS=-Dwarnings`.

### 12.10 Security guide (SECURITY.md, SECURITY-MODEL.md)

SECURITY.md (168 lines) covers: reporting channel (GH Security
Advisories preferred, encrypted email), response SLOs (3 / 7 / 30 / 90
days), scope (auth, IPC, config, secret handling, crypto, filesystem,
proto, SDK, CLI), out-of-scope list, known issues reference to
`SECURITY-AUDIT-FINAL-14042026.md`.

SECURITY-MODEL.md (165 lines) is the structured model: trust boundaries
diagram (line 13-30), untrusted input surfaces (line 32-40).

`docs/book/src/security/secrets.md`, `docs/book/src/security/threat-
model.md`, `docs/book/src/security/audit-dossier.md`,
`docs/book/src/security/model.md` all exist.

**DOC-12-10-01 (HIGH)** — `SECURITY.md:60-61` cites
`crates/pcloud-daemon/src/auth_backend.rs` and
`crates/pcloud-daemon/src/auth_vault.rs` as auth surface. **`auth_backend.rs`
moved to pcloud-backends** (see DOC-12-1-03); `auth_vault.rs` stayed in
pcloud-daemon (that one is correct). Fix the stale half.

**DOC-12-10-02 (LOW)** — `SECURITY.md:9` points at
`SECURITY-AUDIT-FINAL-14042026.md` as the authoritative audit record. I
did not verify existence of that file during this pass — add a check to
the release-checklist that orphaned audit-file references are removed.

### 12.11 Release notes / CHANGELOG.md

CHANGELOG.md is 2028 lines. Format: Keep a Changelog; all entries currently
under `[Unreleased]` (line 15) because no version has been tagged yet. No
`[0.1.0]` section despite workspace version being `0.1.0`
(`Cargo.toml:59`).

**DOC-12-11-01 (LOW)** — With a 2028-line `[Unreleased]` section and no
tagged release, CHANGELOG.md is a dumping ground of per-wave notes, not a
user-facing changelog. The Keep-a-Changelog format expects a cut-off per
release. This is fine pre-alpha, but it should be triaged before the first
tagged release.

**DOC-12-11-02 (LOW)** — CHANGELOG.md:10-13 cites source documents
`FINAL-PARITY-PROOF-WAVE*.md`, `RECONCILIATION-WAVE*.md`,
`SECURITY-AUDIT*.md`, `MATRIX-*.md`, `PARITY-AUDIT-FINAL-14042026.md`. I
did not verify these exist on disk in this pass. If any were purged, the
citations should be purged too.

### 12.12 README quickstart walkthrough

README.md:1-100 covers: feature badge (line 3), workspace layout (line
22-44), build/test/docs commands (line 46-77), daemon + CLI quickstart
(line 82+).

**DOC-12-12-01 (MEDIUM)** — README.md quickstart uses `cargo run -p
pcloud-daemon -- serve` for daemon and `cargo run -p pcloud-cli -- ...`
for CLI. But the actual shipped binary names (per the WiX file,
packaging/macos/README.md, the .deb contents) are `pcloudd` and `pcloudc`.
The README never explicitly tells a reader: "after `cargo install --path
crates/pcloud-daemon && cargo install --path crates/pcloud-cli`, the
binaries are named pcloudd and pcloudc". Remediation: add a one-line
mapping `cargo run -p pcloud-daemon` ↔ `pcloudd`, `cargo run -p
pcloud-cli` ↔ `pcloudc`.

**DOC-12-12-02 (LOW)** — README.md:60 runs `cargo deny --manifest-path
Cargo.toml check`. `audit.toml:10-25` time-boxes **5 advisory ignores**
with `review: YYYY-MM-DD` deadlines (2026-06-01 and 2026-07-15). A
contributor running `cargo audit` after the review dates will (correctly)
see failures. Good hygiene, just flag it.

### 12.13 Cross-cutting — empty-backtick placeholder

**DOC-12-13-01 (MEDIUM)** — Grep shows 10 `.md` files contain the literal
token `` `` `` (empty backticks). Representative hits:
`CONTRIBUTING.md:28` — "Contributions are welcome to the `` workspace";
`CONTRIBUTING.md:38` — "pinned via `rust-toolchain.toml` in ``";
`CONTRIBUTING.md:72` — "All commands run from the `` directory";
`README.md`, `CLAUDE.md`, `SECURITY.md`, `docs/book/src/introduction.md`,
`docs/book/src/parity/status.md`,
`docs/book/src/architecture/overview.md`,
`docs/book/src/architecture/performance.md`,
`docs/book/src/architecture/security-model.md`,
`docs/book/src/security/audit-dossier.md`,
`docs/adr/0001-record-format.md` also hit.

This looks like the aftermath of a global `s/<old_name>/<new_name>/` that
collapsed to empty string. Remediation: grep-replace `` `` `` in all .md
files with the intended project name (probably `pcloud-rs` or
`pcloud-rs-rust-dev`, based on context).

### 12.14 Manpages

`packaging/man/` ships `pcloudc.1`, `pcloudd.1`, `pcloud.conf.5` — good.
I did not open them to verify content matches the current CLI surface.

**DOC-12-14-01 (LOW)** — No CI check that `pcloudc --help` output matches
`pcloudc.1`. Flag.

### 12.15 Plugin documentation

`docs/plugins/README.md` plus `autoheal.md`, `backup-schedule.md`,
`dlp-builtin.md`, `publink-expiry.md` all exist. Crates
(`pcloud-plugin-autoheal`, `pcloud-plugin-backup-schedule`,
`pcloud-plugin-dlp`, `pcloud-plugin-publink-expiry`) are all workspace
members.

Finding: structurally complete; no per-file finding from this dimension.

---

## Summary by Severity

### CRITICAL
- DEP-11-6-01 WiX placeholder UpgradeCode (one-way door for Windows upgrades)
- DOC-12-1-01 ≥41 parity-matrix rows cite moved / non-existent files (undermines bd-1du.10)

### HIGH
- DEP-11-4-01 nfpm hard-coded version drift vs Cargo.toml
- DEP-11-5-04 No macFUSE/fuse-t detection for macOS
- DEP-11-6-02 No CI pipeline for Authenticode MSI signing
- DEP-11-6-03 WiX service runs as `LocalSystem` (unjustified privilege)
- DEP-11-6-04 No WinFSP runtime probe / install-hint
- DEP-11-7-01 FreeBSD rc.d does not preload `fuse.ko`
- DEP-11-9-01 No `dashboards/` — no Grafana JSON, no alert rules
- DEP-11-10-01 No documented `_pcloud_migrations` sentinel query
- DOC-12-1-02 ARCHITECTURE.md crate map missing pcloud-backends
- DOC-12-1-03 SECURITY.md cites non-existent auth_backend.rs path
- DOC-12-10-01 SECURITY.md cites moved auth_backend.rs (same root cause)

### MEDIUM
- DEP-11-1-02 `Type=simple` without `sd_notify` (false-ready race)
- DEP-11-1-03 No `WatchdogSec=` on systemd unit
- DEP-11-2-01 No logrotate.d drop-in for file-based logging
- DEP-11-3-02 AppArmor/SELinux profiles not installed by .deb/.rpm
- DEP-11-4-02 Maintainer address is `example.invalid` placeholder
- DEP-11-4-03 postinstall/postremove scripts unaudited
- DEP-11-5-01 launchd plist missing `ExitTimeOut`
- DEP-11-5-02 launchd plist uses `--system` flag that may be unhandled
- DEP-11-5-03 Notarization pipeline exists but no CI wiring
- DEP-11-6-05 Windows worker panic hides real exit code from SCM
- DEP-11-7-02 FreeBSD rc.d declares pcloudd_user but never uses it
- DEP-11-9-02 No `/livez` vs `/readyz` distinction
- DEP-11-9-03 OTLP pipeline never run against live collector in CI
- DEP-11-10-02 No auth-vault format version byte
- DOC-12-1-04 CLAUDE.md itself cites stale backend paths
- DOC-12-3-01 STATUS.md hand-edited — no regen script
- DOC-12-5-01 mdBook `git-repository-url` points at upstream C tree
- DOC-12-7-01 Runbook references non-existent APT/DNF/Nix repos
- DOC-12-7-02 Runbook service name `pcloud-daemon` vs shipped `pcloudd`
- DOC-12-7-03 No mount walkthrough (pending bd-1du.4) — mention it
- DOC-12-8-01 No FUSE-refused troubleshooting section
- DOC-12-8-02 No TLS cert mismatch quick-ref
- DOC-12-12-01 README uses `cargo run` names, not installed binary names
- DOC-12-13-01 10+ .md files contain empty-backtick placeholder `` `` ``

### LOW
- DEP-11-1-01 systemd `Documentation=` URL points at C upstream
- DEP-11-1-04 IPAddressAllow= hostname resolution hazard
- DEP-11-1-05 Two competing systemd units with different hardening
- DEP-11-3-01 SELinux policy_module version not tied to release
- DEP-11-4-04 No explicit RPM scriptlet conventions
- DEP-11-4-05 No distinct RPM `%pre/%post` handling
- DEP-11-5-05 Dead `PCLOUD_*` env vars in macOS plist
- DEP-11-6-06 WiX `StageDir` not documented
- DEP-11-7-03 OpenBSD/NetBSD rc.d unverified scaffolding
- DEP-11-8-01 No `config.example.toml` shipped with .deb
- DEP-11-8-02 Env-var docs duplicated 4 places
- DEP-11-11-01 No mount-orphan registry in backup docs
- DEP-11-13-01 No server-profile systemd drop-in
- DOC-12-5-02 mdbook not enforced in CI
- DOC-12-5-03 Two security-model docs — drift risk
- DOC-12-7-04 Runbook `cd .` is a developer path
- DOC-12-8-03 No sync-queue-stuck troubleshooting
- DOC-12-10-02 SECURITY-AUDIT file reference unverified
- DOC-12-11-01 CHANGELOG a dumping ground under `[Unreleased]`
- DOC-12-11-02 CHANGELOG cites MATRIX-*.md / WAVE-*.md files unverified
- DOC-12-12-02 `cargo audit` will fail after time-boxed ignores expire (by design, but flag)
- DOC-12-14-01 No CI check that manpages match `--help` output

### NONE (honest-and-correct)
- FIPS claims (§11.14)
- Matrix ↔ Review alignment (§12.2)
- REJECTED-RATIONALES coverage (§12.4)
- CLAUDE.md honesty hygiene (§12.6) — the project polices its own rules unusually well

---

## Key cross-cutting observations

1. **The single biggest documentation defect is the stale backend path
   citations (DOC-12-1-01).** It affects the parity matrix, the review
   narrative, CLAUDE.md, ARCHITECTURE.md, API-REFERENCE.md, and
   SECURITY.md. Fixing it is mechanical but blocks `bd-1du.10`.
2. **The systemd unit (packaging/systemd/pcloudd.service) is
   unusually-strong** — substantially above average for a pre-alpha fork.
   Three directives are missing (`WatchdogSec=`, `Type=notify`+`sd_notify`,
   `ExitTimeOut` analogue on macOS) but the rest is production-shape.
3. **The Windows UpgradeCode placeholder (DEP-11-6-01) is a ticking
   time bomb.** Any MSI that ships with the placeholder GUID cannot be
   upgraded by a later MSI with a real GUID.
4. **Dashboards are entirely absent (DEP-11-9-01).** Shipping
   Prometheus metrics without a Grafana dashboard and alert rules is
   half-finished operational work.
5. **Honesty discipline is genuinely strong.** Self-policing of the
   "no full-parity / no production-ready" rule across 10+ files is
   atypical. Operators reading the docs get an accurate picture of the
   project's maturity.

---

_End of Section 11-12 audit._

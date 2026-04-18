# Sections 11 & 12: Deployment & Documentation
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 11/12)

## Findings

### CRITICAL [1]
- **C-1**: `packaging/windows/wix/pcloud-rs.wxs:14` — hard-coded placeholder
  `UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"`. A WiX product
  that ships with a placeholder UpgradeCode will lock the installer family
  to an illegal GUID forever; upgrades, uninstalls, and `MajorUpgrade`
  detection silently break. The comment even notes "must stay stable
  forever after", yet no real GUID is committed. Remediation: generate a
  real GUID once, check it in, delete the TODO. Block release builds until
  this is fixed (a CI gate would help).

### HIGH [7]
- **H-1**: `packaging/systemd/pcloudd.service:21` — `Type=simple` declared,
  no `Type=notify`, no `NotifyAccess=`, no `WatchdogSec=`. The unit's
  own comment (lines 12-18) acknowledges `sd_notify(3) READY=1` is never
  emitted. Grep across `crates/` returns **no** `NOTIFY_SOCKET` /
  `sd_notify` reference anywhere. Impact: systemd cannot tell when the
  daemon is actually ready, so dependent units race, and there is no
  watchdog-based crash recovery — a core systemd hardening primitive is
  unused. Remediation: wire `libsystemd` (or a pure-Rust `sd-notify`
  crate) and flip the unit to `Type=notify` + `WatchdogSec=30s`.

- **H-2**: `packaging/debian/nfpm.yaml:16` — maintainer is the
  placeholder `"pcloud-rs maintainers <maintainers@example.invalid>"`.
  The same sentinel appears verbatim in `packaging/debian/control:2`. A
  Debian package with an `@example.invalid` maintainer fails `lintian`
  policy and is rejected by every serious mirror. Remediation: replace
  with a real maintainer address before any `.deb` upload.

- **H-3**: `packaging/macos/com.pcloud.pcloud-rs.plist` and
  `com.pcloud.pcloudd.plist` — both plists lack `ExitTimeOut`,
  `ThrottleInterval`, and `SoftResourceLimits`. `KeepAlive` only covers
  crash/success but no `NetworkState` gate, and `SessionCreate` is
  absent for the LaunchAgent. Also no notarization pipeline beyond
  `packaging/signing/notarize-macos.sh`; `docs/book/src/operations/
  packaging-matrix.md` §1 explicitly admits "macOS `.pkg` notarisation
  is pending an active Apple Developer ID". Remediation: add the
  missing launchd keys and a CI job stub that runs `notarytool`.

- **H-4**: `packaging/freebsd/pcloudd.rc` — the script lacks
  `daemon_user`/`daemon_chdir` / `daemon_pidfile` supervision, never
  calls `kldload fusefs`, and does not declare `required_modules=fusefs`.
  `bd-1du.4` explicitly calls mount on FreeBSD an "open proof". Running
  `service pcloudd start` on a vanilla 13.2 host without `fusefs` loaded
  will fail to mount with an opaque error. Remediation: add
  `required_modules="fusefs"` and a `pcloudd_prestart() { kldload -n
  fusefs; }` hook, plus `pcloudd_user` honoured by `daemon_user`.

- **H-5**: CI platform coverage is weak. `.github/workflows/ci.yml:36-54`
  runs macOS and Windows jobs but **excludes `pcloud-fs`** (`--exclude
  pcloud-fs`). The FreeBSD block is commented out (lines 56-67). Impact:
  the only live-tested mount path is Linux, and nothing in CI exercises
  the Windows service wrapper or the macOS LaunchAgent plist. Combined
  with `packaging-matrix.md` §1 honesty callout ("Linux is the only
  live-tested mount path"), shipping packages for non-Linux is a trust
  fall. Remediation: add cross-platform-actions FreeBSD runner; install
  `winfsp` on the Windows runner and lift the `pcloud-fs` exclusion.

- **H-6**: `packaging/windows/wix/pcloud-rs.wxs` — `ServiceInstall
  Account="LocalSystem"` (line 67). A pCloud sync daemon running as
  `LocalSystem` is a significant privilege escalation risk: any daemon
  vulnerability gives `NT AUTHORITY\SYSTEM`, which is far above the
  `0600`/owner-only posture the Linux path upholds. The `SECURITY.md`
  principle ("secrets belong to the UID") is contradicted by
  `LocalSystem`. Remediation: run the service under a dedicated
  low-privilege account (create via `net user /add pcloud-rs-svc` and
  `ServiceInstall Account="NT SERVICE\pcloud-rs-svc"`), or run as the
  interactive user.

- **H-7**: `packaging/debian/nfpm.yaml:35-37` — the binaries are
  installed as `/usr/bin/pcloud-rs` and `/usr/bin/pcloudd`, but the
  systemd unit references `/usr/local/bin/pcloudd`
  (`packaging/systemd/pcloudd.service:22`) and the AppArmor profile
  pins `/usr/local/bin/pcloudd` (`packaging/apparmor/usr.local.bin.
  pcloudd:11,18`). An nfpm-built `.deb` will install the binary where
  the service unit and AppArmor profile cannot see it. Remediation:
  pick one (`/usr/bin` for Debian FHS compliance) and rewrite the unit
  + AppArmor profile to match, or add an explicit symlink.

### MEDIUM [8]
- **M-1**: `OPERATIONS-RUNBOOK.md:20` documents a flag that does not
  exist: `target/release/pcloud-daemon --config ~/.config/pcloud-rs/
  config.json`. The systemd unit invokes `/usr/local/bin/pcloudd
  serve`, with config discovered via `PCLOUD_ROOT`. No `--config` flag
  is documented in `crates/pcloud-config/src/env.rs`. This is a docs vs
  code drift that misleads operators. Remediation: replace with the
  actual invocation.

- **M-2**: `/livez` and `/readyz` are referenced in the audit brief and
  in the two prior audits under `.audits/01` and `.audits/02` but do
  **not exist**. `crates/pcloud-web/src/routes.rs:73,89` only exposes
  `GET /health` returning static `"ok"`. It never queries the daemon
  state, so it is not a real readiness probe (the doc-comment even
  admits "Never touches the daemon"). `crates/pcloud-observability/
  src/exporter.rs:275` has a second `/health` that is richer but still
  not Kubernetes-compliant (no separate `/readyz` path). Remediation:
  add `/livez` (process alive) and `/readyz` (IPC ping + vault open +
  SQLite reachable) with proper 503 semantics before ready.

- **M-3**: No Grafana/Prometheus dashboards anywhere. Grep for
  `dashboards|grafana` returns **no files**. Operational monitoring
  is half-implemented: the daemon renders Prometheus exposition (per
  `exporter.rs`), but there is no shipped dashboard JSON, no alert
  rule file, no SLO-burn PromQL. `OPERATIONS-RUNBOOK.md` and
  `docs/enterprise/tracing.md` describe concepts but ship no config.
  Remediation: commit a `dashboards/` directory with at least one
  Grafana dashboard and a Prometheus alert ruleset.

- **M-4**: No OpenTelemetry exporter. `crates/pcloud-observability/
  src/tracing.rs` does structured tracing but does not speak OTLP.
  `docs/enterprise/tracing.md` (15 KB) describes tracing as an
  enterprise capability but the code does not emit spans over OTLP/gRPC
  or OTLP/HTTP. Remediation: add an `opentelemetry-otlp` feature or
  mark the enterprise doc as roadmap-only.

- **M-5**: `packaging/selinux/pcloud-rs.te:20-33` defines a complete
  policy but `packaging/selinux/pcloud-rs.fc` was not inspected in
  depth and there is **no `Makefile` wrapper** to build the .pp module;
  the `.te` file expects `make -f /usr/share/selinux/devel/Makefile
  pcloud-rs.pp` (line 8) to just work. No build artifact, no RPM spec
  that installs the policy. Remediation: ship a pre-built `.pp` (or
  have `rpmbuild` produce one) and add a `semodule -i` line to the
  RPM postinstall.

- **M-6**: `C_FEATURE_PARITY_MATRIX.csv` has **158 Implemented + 28
  Rejected = 186 rows + 1 header line**, confirmed with `awk -F','`.
  `STATUS.md` line 68 says the same "158 / 0 / 0 / 28". But
  `CLAUDE.md:76-78` still lists `bd-1du.4` "Replace filesystem shell
  with real mounted-drive parity" as open, and `bd-1du.10` "Prove and
  gate final C parity claims" as open. `STATUS.md:179-180` flipped
  Row 85 (mounted filesystem) to Implemented on 2026-04-16, but the
  open `bd-1du.4` bead contradicts that. There is an internal
  inconsistency: either row 85 is Implemented (bead should close) or
  `bd-1du.4` is still open (row should be `Partial`). Remediation:
  reconcile STATUS / matrix / bead tracker in one direction before
  the next release tag.

- **M-7**: `CHANGELOG.md` has **no tagged releases** — everything is
  under `[Unreleased]` (line 15). `README.md:3` shows a `Feature
  Surface` badge but no `version` badge, and `Cargo.toml:59` pins
  `version = "0.1.0"` with `publish = false` (line 61). A pre-alpha
  that ships nothing means all the package recipes (deb, rpm, dmg,
  msi) are untested end-to-end. Remediation: cut a real `0.1.0-alpha.1`
  tag and exercise every packaging pipeline against it.

- **M-8**: `scripts/check-versions.sh` compares `Cargo.toml` vs
  `nfpm.yaml` only. It does **not** check `packaging/snap/
  snapcraft.yaml` (version `'0.1.0'`), `packaging/flatpak/com.pcloud.
  pcloud-rs.yaml`, the WiX product version, or the Homebrew formula.
  All of these drift independently today. Remediation: extend the
  check to every packaging manifest and wire into CI.

### LOW [8]
- **L-1**: `packaging/systemd/pcloudd.socket:3` still cites the legacy
  upstream `Documentation=https://github.com/pcloudcom/console-client`,
  whereas `pcloudd.service:3` points to the fork
  `https://github.com/ezechiel203/pcloud-rs`. Inconsistent cross-ref.
  Remediation: harmonise to the fork URL.

- **L-2**: `docs/book/src/getting-started/install.md:13` labels the
  project as "**pre-alpha**" while `README.md:3` boasts a large
  Feature-Surface badge that reads more like a "ready" status. The
  badge's green colour will mislead. Remediation: use an `orange`
  badge labelled "pre-alpha" instead.

- **L-3**: `packaging/apparmor/usr.local.bin.pcloudd:50-54` has the
  FUSE mount block commented out. Since `bd-1du.4` is still open, this
  is understandable, but users who enable FUSE today will be silently
  blocked by AppArmor. Remediation: document the un-comment step in
  `docs/book/src/operations/platforms/linux.md`.

- **L-4**: `packaging/init/systemd/pcloudd.service` (ExecStart at
  `/usr/local/libexec/pcloudd-wrapper.sh`) is a **second, divergent**
  systemd unit from `packaging/systemd/pcloudd.service` (ExecStart at
  `/usr/local/bin/pcloudd serve`). One uses `DynamicUser=yes`, the
  other `User=pcloud-rs`. Operators will load the wrong one and debug
  for hours. Remediation: pick one canonical unit, delete or rename
  the other.

- **L-5**: `packaging/debian/postinst:18-20` prints a message saying
  "To start the daemon manually: systemctl --user start
  pcloudd.service" — but the deb installs a **system** unit at
  `/lib/systemd/system/pcloudd.service`, not a user unit. The advice
  is incorrect.

- **L-6**: `docs/book/book.toml` has no `[preprocessor.linkcheck]`
  section. Broken inline links (I spotted `SUMMARY.md` lines 46, 76-85,
  89-92 cross into `../../`-escaping paths like `../../parity/...`
  and `../../enterprise/...` which mdBook emits warnings for)
  will not be caught. Remediation: add `mdbook-linkcheck` to CI.

- **L-7**: `docs/book/src/SUMMARY.md:49` has an empty-link chapter
  `[Platforms]()` with subchapters. mdBook accepts this but the
  chapter is un-navigable. Should be a real page or removed.

- **L-8**: `CLAUDE.md:76` still cites `CLAUDE.md` explicitly mentions
  legacy C sources as "historical — they point to the upstream
  `pcloud-rs` C tree". The upstream project referenced in
  `CLAUDE.md:27` is typo'd — it says `github.com/pcloudcom/pcloud-rs`
  (the fork) but the historical upstream is actually
  `github.com/pcloudcom/pcloudcc`, as correctly stated in
  `README.md:9`. Remediation: fix the URL in CLAUDE.md.

## Section 11: Deployment & Operations — Detailed

### Linux systemd
`packaging/systemd/pcloudd.service` is well-hardened (most
`Protect*=yes` flags, `SystemCallFilter`, `MemoryMax=512M`,
`DynamicUser=yes`, `RuntimeDirectoryMode=0700`) but has three
weaknesses:
1. **No `Type=notify` + `WatchdogSec=`** (H-1): systemd cannot
   supervise startup readiness or crash recovery.
2. **Two divergent units** in the tree (`packaging/systemd/` and
   `packaging/init/systemd/`) with different `ExecStart` paths (L-4).
3. **`ExecStart=/usr/local/bin/pcloudd`** but nfpm installs to
   `/usr/bin/` (H-7).

`packaging/systemd/pcloudd.socket` is fine (owner-only `0600`,
`DirectoryMode=0700`, `PassCredentials=yes`, `MaxConnections=32`).

### Log rotation
`packaging/debian/pcloud-rs.logrotate` is correct (daily, rotate 14,
`compress delaycompress`, `postrotate systemctl kill -s HUP`).
Group assumption (`pcloud-rs:pcloud-rs`) will mismatch
`DynamicUser=yes` — operators must either drop `DynamicUser` or
change the logrotate `create` mode.

### SELinux / AppArmor
Both profiles present (`packaging/selinux/pcloud-rs.te`,
`packaging/apparmor/usr.local.bin.pcloudd`). Profiles are
tight (deny `/etc/shadow`, `/root/**`, `capability sys_module`,
`ptrace`) and well-commented. Missing: SELinux build Makefile
wrapper (M-5), integration test for `aa-enforce`.

### Debian packaging
`nfpm.yaml` present, has `depends` list and maintainer scripts
pointing to `postinst` / `postrm`. Gaps: placeholder maintainer
email (H-2), postinst message says "systemctl --user start"
while the unit is system-scoped (L-5), path mismatch with systemd
unit (H-7). No `debian/copyright`, no `debian/changelog`, no
`debian/rules`.

### macOS launchd
Two plists (`com.pcloud.pcloud-rs.plist` LaunchAgent,
`com.pcloud.pcloudd.plist` LaunchDaemon) are well documented
inline, honest about unread env vars, include `RunAtLoad`,
`KeepAlive` subset, `ProcessType`. Missing: `ExitTimeOut`,
`ThrottleInterval`, `SoftResourceLimits`, and a CI notarisation
pipeline (H-3). `packaging/signing/notarize-macos.sh` exists but
is not wired into any workflow.

### FreeBSD rc.d
`packaging/freebsd/pcloudd.rc` is a standard `rc.subr` script.
Gaps (H-4): no `required_modules=fusefs`, no `daemon_user`
wiring, no `kldload` prestart, no `command_args=-p ${pidfile}`
is acceptable only if the daemon binary supports `-p` (not
verified). NetBSD/OpenBSD skeletons exist (`packaging/netbsd/`,
`packaging/openbsd/`) as `packaging/init/{netbsd,openbsd}/`
but nothing inspected live.

### Windows
Two critical gaps: placeholder UpgradeCode (C-1), and service
runs as `LocalSystem` rather than a dedicated low-priv account
(H-6). WinFSP is declared as a `PackageDependency` which is
correct. `packaging/windows/wix/pcloud-rs.wxs` lacks Authenticode
EV signing hook (line 4 TODO comment admits this). CI never
builds the MSI. `crates/pcloud-daemon-win/` contains a single
`main.rs` — the SCM wrapper is thin.

### Configuration
`crates/pcloud-config/src/env.rs` is comprehensive and
well-documented (every PCLOUD_* env var table-documented).
`schema.rs` is 42 KB (documented fields). Example config is
shipped — yes, per `docs/book/src/reference/config.md`. The
`PCLOUD_ROOT` / `PCLOUD_ENV` hierarchy is consistent between
the plist, systemd unit, and the loader.

### Observability
Prometheus exposition via `crates/pcloud-observability/src/
exporter.rs`. Metrics registry at `metrics.rs` (28 KB). Health
reports via `health.rs` and IPC `Method::Health`. Gaps: no OTLP
(M-4), no Grafana dashboards (M-3), `/livez`/`/readyz` not
implemented (M-2). `slo.rs` (36 KB) defines SLOs but nothing
exports them to an external TSDB beyond the Prometheus scrape
endpoint.

### Upgrade path
SQLite migrations: `crates/pcloud-store/src/migrations.rs`
implements forward-only `MigrationPlan` (lines 15-118) with 11
schema versions (V1..V11), `PRAGMA user_version` gate, crash-
safe per-step commit, explicit `BackwardsMigration` error.
This is enterprise-quality. Auth vault format versioning: not
inspected in depth, but the runbook (line 75) mentions journal
roll-forward.

### Health checks
`GET /health` exists twice (pcloud-web and pcloud-observability)
but returns static "ok" (web) or polls a `HealthSnapshot` shell
bit (observability). Neither probes all dependencies (SQLite,
vault, IPC listener, API reachability). No `/livez`/`/readyz`
split.

### Resource limits
systemd unit: `MemoryMax=512M`, `MemoryHigh=384M`,
`CPUQuota=75%`, `TasksMax=256`, `LimitNOFILE=4096`,
`LimitNPROC=256`, `LimitCORE=0`. Good cgroup integration.
FreeBSD/macOS have no equivalent — the FreeBSD rc.d does not
set `limits -P` or login.conf class, macOS plists have no
`SoftResourceLimits`.

### FIPS
Not claimed. `docs/book/src/architecture/security-model.md:283`
mentions FIPS once in a rejection context ("have no FIPS
constraint"). Good — do not claim what is not built.

## Section 12: Documentation Quality — Detailed

### Parity docs accuracy (spot-check)
- Matrix: **158 Implemented + 28 Rejected = 186 rows** (header
  row separate). Confirmed with `awk -F','`. Matches `STATUS.md`
  claim (158/0/0/28). Good.
- Spot-check 5 rows:
  - **Row 15 `psync_set_user_pass`** → `crates/pcloud-proto/src/
    auth_api.rs:115` — path exists. OK.
  - **Row 76 `psync_stat_path`** → per STATUS.md line 167 was
    flipped to Implemented on 2026-04-16 via schema v11. Verified:
    `crates/pcloud-store/src/schema.rs` imports `SCHEMA_VERSION_V11`
    at line 7-13 of `migrations.rs`. Consistent.
  - **Row 85 `mounted pcloud filesystem`** → marked Implemented
    but `bd-1du.4` is still open (M-6 finding).
  - **Row 8 `psync_get_notifications`** → points at
    `crates/pcloud-proto/src/notifications_api.rs:80` + daemon
    + SDK. Plausible.
  - **Row 11 `psync_get_status`** → `crates/pcloud-daemon/src/
    runtime.rs:1008` — plausible at file level.
- Rejected rationale coverage: `REJECTED-RATIONALES-14042026.md:5`
  names exactly 28 rows (matrix rows 2, 5, 6, 10, 12, 13, 43, 44,
  45, 46, 99-106, 113-115, 126, 151-152, 157, 160, 167, 169) —
  matches the 28 Rejected count. Doc is well-categorised (Ghost /
  Stub / Replaced / etc.).

### STATUS.md
Up to date (2026-04-16 entries at top). Counts internally
consistent. Only issue: the Row-85 flip vs the still-open
`bd-1du.4` bead (M-6).

### CLAUDE.md
Honesty callouts are explicit (lines 52-60: do NOT claim full
parity / production-ready / enterprise-ready / drop-in). But
the doc has two issues:
- Historical upstream URL typo vs README (L-8).
- Claim in `CLAUDE.md:174` that "mounted-drive / FUSE proof"
  remains open contradicts the matrix claim of Row 85
  Implemented.

### Book
`docs/book/book.toml` is minimal (no linkcheck preprocessor,
L-6). `SUMMARY.md` has an empty-link `[Platforms]()` entry
(L-7). Chapters exist for: introduction, getting-started (3
files), architecture (6 files), security (4 files), operations
(9 files + 3+ platform subpages), development (6 files),
reference (5 files). No `cargo doc` output is linked from the
book; no SDK API reference beyond inline `///` rustdoc.

### Deployment guide
`docs/book/src/operations/deployment.md` is a serious document
(honesty callout at line 17). A senior sysadmin can follow it
provided they correct the `--config` runbook flag (M-1), ignore
the placeholder maintainer, pick one systemd unit (L-4), and
are willing to accept that Linux is the only live-tested mount
path. For macOS/Windows/BSD, the guide is roadmap-only.

### SDK API reference
No `cargo doc` check attempted, but `CHANGELOG.md` entry
(Four-parallel-closures) mentions that doc-gate `RUSTDOCFLAGS=
-Dwarnings cargo doc --workspace --no-deps` **passes**, so
broken intra-doc links are caught. Good. No external rendered
rustdoc link in the README.

### Security guide
`SECURITY.md` (230 lines) is a proper responsible-disclosure
policy. `SECURITY-MODEL.md` exists at repo root.
`docs/book/src/security/` has `model.md`, `secrets.md`,
`threat-model.md`, `audit-dossier.md` — four chapters. Secrets-
handling user guidance is present (`secrets.md`). Good.

### Release notes / CHANGELOG
Everything is `[Unreleased]` (M-7). No semver tag has shipped,
which matches the pre-alpha status but means packaging recipes
are untested. Entries inside `[Unreleased]` are well-formatted
and dated (2026-04-16 entries at top).

### README quickstart
`README.md` lines 48-132 provide clone → build → run → auth
commands, but the quickstart is Linux-centric. `cargo run -p
pcloud-daemon -- serve` may work on Linux only; macOS needs
fuse-t, Windows needs WinFSP. Mount command is absent from the
quickstart, only `crypto`/`sync`/`account`/`download` are
shown — consistent with `bd-1du.4` still open.

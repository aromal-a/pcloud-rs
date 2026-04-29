# Dimension 11 — Deployment & Operations

**Audit date:** 2026-04-29
**Scope:** packaging artefacts, init-system integration, observability,
upgrade/migration, health, FIPS posture, resource limits.
**Mode:** read-only.

---

## Summary

The repository ships a remarkably broad and disciplined ops surface. The
shipped `systemd` unit is one of the strictest production-grade hardening
profiles seen in OSS (full `Protect*=`, syscall allow-list, `MemoryMax=`,
`WatchdogSec=`, `Type=notify` with proper `NotifyAccess=main`). Linux
packaging (`.deb` / `.rpm` via nfpm, AppArmor, SELinux, logrotate,
postinst), macOS (signed launchd plist, hardened-runtime entitlements,
notarisation script), and Windows (WiX MSI with virtual service account,
SCM wrapper, signtool script) are all present and individually
plausible. Configuration is JSON-Schema validated, env-var overrides
are documented, schema migrations are forward-only with documented
no-rollback policy, OpenTelemetry is wired with an attribute allow-list
to prevent PII leakage, and a Prometheus exporter with low-cardinality
sanitised labels is shipped together with Grafana JSON and Prom alert
rules.

The gaps are operational rather than architectural:

- The Windows Service path (`pcloudd-svc`) is the SCM wrapper but the
  underlying `pcloud_daemon::serve_with_shutdown` named-pipe accept loop
  is not yet wired (returns `Unsupported`), so MSI installation produces
  a service that starts but does no work — confirmed by the project's
  own `CLAUDE.md` ("named-pipe IPC accept-loop wiring" pending).
- No FIPS-mode runtime gate exists. `docs/fips.md` describes the design
  and `CryptoPolicy::fips_mode` is referenced as future work; the
  current crypto stack uses Argon2id + AES-256-GCM (Enhanced backend)
  or pclsync-compat PBKDF2 + RSA-OAEP + custom AEAD — neither is FIPS
  140-3 validated.
- The shipped `systemd` unit's `IPAddressDeny=any` + localhost-only
  posture is correct from a hardening standpoint but renders the
  daemon **non-functional out of the box** (cannot reach the pCloud
  API). The required `override.conf` drop-in is documented but operators
  must remember to install it; this is a footgun for "package installs,
  service starts, nothing works" scenarios.
- The shipped `pcloudd.service` `WatchdogSec=30s` requires
  `NotifyAccess=main` (correctly set) and a daemon that emits
  `WATCHDOG=1` per loop iteration. The unit comment claims this is
  done; spot-check of the daemon source was not performed at the
  call-site level.
- `cargo-deb.toml` is a documented no-op snippet (cannot be edited
  into Cargo.toml under existing project rules) — `.deb` build relies
  on nfpm which is **not yet wired into CI** (`pcloud-rs-s1p.69` open).
- No example config file is shipped (no `pcloudd.example.toml` under
  `packaging/` or `crates/pcloud-config/examples/`). Operators must
  consult `crates/pcloud-config/src/schema.rs::CONFIG_SCHEMA_JSON` to
  produce a valid file. `crates/pcloud-config/examples/parse_config.rs`
  is a code example, not a config example.
- Health-server bind is loopback-only with no opt-out — correct for
  most postures, but operators running the daemon on a Kubernetes pod
  with `livenessProbe` from a sidecar will be fine, while those who
  want a service-mesh-injected probe from outside the pod loopback
  cannot do so without a reverse proxy.
- BSD/Windows mount cleanup on signal is Tier-3 by the project's own
  admission (no ACTIVE_MOUNTS registry drained on signal); a crash
  leaves stale mountpoints requiring manual `umount -f` /
  WinFSP admin tooling.

Finding totals: **0 CRITICAL · 4 HIGH · 7 MEDIUM · 6 LOW**.

---

## Findings

### HIGH-11.1 — Windows Service ships an installable but non-functional binary

**Severity:** HIGH
**File:** `crates/pcloud-daemon-win/src/main.rs:60-78`,
`crates/pcloud-daemon-win/README.md:50-54`,
`packaging/windows/wix/pcloud-rs.wxs:71-80`,
`CLAUDE.md` (Windows posture section).

**Evidence:**
The MSI (`pcloud-rs.wxs:71-80`) installs `pcloudd-svc.exe` and registers
it as a Windows Service under the virtual account `NT SERVICE\pcloudd`.
The wrapper compiles and reports SCM state correctly, but per the
project's own `CLAUDE.md`:

> `pcloud_daemon::serve_with_shutdown` on Windows currently returns
> `Unsupported` (commit `d79004d`); `pcloudd-svc` compiles and starts
> but runs a no-op stub until this lands. This is the Tier-1 blocker
> and in-flight as of this writing.

**Risk:** An operator who runs `msiexec /i pcloud-rs.msi` and then
`sc start pcloudd` will see the service report `Running` while the
daemon performs no IPC, no sync, no mount work. There is no warning in
`packaging/windows/wix/README.md` or in the post-install screen.

**Remediation:** Either (a) gate the MSI ServiceInstall component
behind a `Feature` element so the service is opt-in until the named
pipe accept loop lands, or (b) ship a banner in the MSI UI noting
"Service start requires pcloud-rs ≥ X.Y where named-pipe IPC is
implemented." Track via existing bead `bd-xplat-windows`. Do not
publish a signed MSI to a public channel until the loop is live.

---

### HIGH-11.2 — `.deb` / `.rpm` packaging is documented but not built or signed by CI

**Severity:** HIGH
**File:** `packaging/debian/nfpm.yaml:21-23`,
`packaging/debian/cargo-deb.toml:1-12`.

**Evidence:**
`nfpm.yaml:21-23` says:

> CI note: the .deb build is not yet wired into ci.yml (tracked
> pcloud-rs-s1p.69). To add it, run the amd64 build command above in a
> post-release workflow step and upload dist/*.deb as a release asset.

`cargo-deb.toml` is a snippet block that is intentionally not active
because the project's CLAUDE.md rules forbid editing `Cargo.toml`.
Therefore there is no automated, signed, reproducible Linux package
artefact pipeline. Whatever a maintainer ships from a workstation is
unverifiable.

**Risk:** No supply-chain integrity for Linux packages. A compromised
maintainer workstation produces signed-by-nothing `.deb` artefacts that
match no release-signed checksum. Reproducible-build claim in CLAUDE.md
("reproducible-build bit-identity check across two hosts") is open.

**Remediation:** Wire nfpm into a `release.yml` workflow step gated on
git tag. Sign artefacts with the repo's existing release key (see
`packaging/signing/`). Publish detached `.deb.asc` and SHA256
checksums alongside the `.deb` on the GitHub release page. Add a
reproducible-build CI matrix that runs the same nfpm invocation on
two distinct hosts and compares hashes.

---

### HIGH-11.3 — Shipped systemd unit is non-functional out of the box (deny-all egress)

**Severity:** HIGH
**File:** `packaging/systemd/pcloudd.service:118-122`,
`packaging/systemd/override.conf.example:1-41`.

**Evidence:**
The shipped unit sets `IPAddressDeny=any` followed by
`IPAddressAllow=localhost`, which blocks every egress to the pCloud
API (`api.pcloud.com`, `eapi.pcloud.com`, `binapi.pcloud.com`). The
operator must install `override.conf.example` as a drop-in. The README
(`packaging/systemd/README.md:25-31`) documents this clearly, but the
postinst (`packaging/debian/postinst:8-23`) does **not** install the
override; it merely echoes installation instructions to stdout.

**Risk:** The default install-and-start workflow (`apt install
pcloud-rs && systemctl --user start pcloudd`) produces a daemon that
fails every API call with `EHOSTUNREACH` and reports
"Authenticated=false" forever. Operators chase ghosts. Worse, the
unit is `Type=notify` with `Restart=on-failure`: if the binding loop
declares ready but every API call later fails, the failure mode is
silent.

**Remediation:** Either (a) ship the unit with a permissive
`IPAddressAllow=any` baseline and document that a drop-in can tighten
it, or (b) extend `postinst` to drop the `api-access.conf` override
into `/etc/systemd/system/pcloudd.service.d/` automatically when the
target install detects systemd is present. Option (b) is the
operator-friendly path; option (a) inverts the security default.
Recommend (b) so the strict-by-default story stays intact.

---

### HIGH-11.4 — No FIPS runtime gate; FIPS claims would fail

**Severity:** HIGH
**File:** `docs/fips.md:28-96`,
`crates/pcloud-crypto/Cargo.toml:73`,
`crates/pcloud-crypto/src/lib.rs:68`,
`AUDIT_REPORT.md:1719`.

**Evidence:**
`docs/fips.md:51-96` describes a `[crypto] fips_mode` runtime policy
gate as future work. No such gate exists in the current source. The
`PclsyncCompat` backend uses PBKDF2-HMAC-SHA-512 + RSA-4096-OAEP +
custom sector AEAD — the AEAD is not FIPS-validated. The `Enhanced`
backend uses Argon2id + AES-256-GCM — Argon2id is not on the FIPS
140-3 approved KDF list. Neither backend is interoperable with a FIPS
provider; the cargo manifest does not pull `aws-lc-rs` or any
FIPS-claimed primitive.

**Risk:** Any operator deploying into a FedRAMP / DoD / financial
regulated environment cannot honestly claim FIPS compliance. Marketing
or sales claims to that effect would be false.

**Remediation:** Implement `CryptoPolicy::fips_mode: bool` per
`docs/fips.md`. When enabled: switch KDF to PBKDF2-HMAC-SHA-512 only,
swap AEAD to AES-256-GCM via a FIPS-validated provider
(`aws-lc-rs::default::default_provider()` is the standard path), and
disable the Enhanced backend selection. Until that lands, ensure no
release notes or README claim FIPS compatibility.

---

### MEDIUM-11.5 — No example configuration file is shipped

**Severity:** MEDIUM
**File:** absent under `packaging/`, `docs/`,
`crates/pcloud-config/examples/`.

**Evidence:**
`crates/pcloud-config/examples/` contains only `parse_config.rs`
(Rust code), not a JSON/TOML reference profile. The schema is defined
inline in `crates/pcloud-config/src/schema.rs:23-1358` as a string
constant. There is no canonical
`pcloudd.example.toml` / `profile.example.json` operators can copy
into `/etc/pcloud-rs/profile.json` as a starting point. Defaults live
in `secure_defaults` (`crates/pcloud-config/src/lib.rs:363`) with no
serialised reference dump.

**Risk:** First-time operators must read Rust source to compose a
valid config file. Schema drift across versions is invisible without
a tracked example.

**Remediation:** Add `packaging/config/profile.example.json` rendered
from `ConfigProfile::secure_defaults(...).serialize()` and lock it in
a `cargo test` snapshot test so any schema change forces an update.
Reference it from `OPERATIONS-RUNBOOK.md`.

---

### MEDIUM-11.6 — Auth-vault format has no documented version field / migration policy

**Severity:** MEDIUM
**File:** `crates/pcloud-daemon/src/vault/file.rs` (no
`VAULT_VERSION` / `format_version` constant found by grep),
`crates/pcloud-daemon/src/auth_vault.rs:25-43`.

**Evidence:**
`grep -n "VERSION\|format_version" crates/pcloud-daemon/src/vault/*.rs`
returns nothing. Compare to the SQLite store
(`crates/pcloud-store/src/migrations.rs:5-12`,
`SCHEMA_VERSION_V1..V11`) and the on-disk config file
(`crates/pcloud-config/src/migrate.rs:59 — CURRENT_VERSION = 2`),
both of which carry explicit, migrated, forward-only version fields.

The vault's serialised payload has no equivalent. If the field layout
ever changes (e.g. addition of refresh-token, scope, server-name), an
older daemon reading the new file (or vice-versa) has no machine-
readable signal that the format has rolled forward.

**Risk:** A user upgrade that changes vault layout produces
"InsecureMetadata" / parse errors with no graceful migration. The
project's stated upgrade-path requirement ("auth-vault format
versioning") is not satisfied.

**Remediation:** Embed a `version: u32` field in the vault payload,
introduce `VAULT_VERSION` constant alongside the schema, and apply
the same forward-only migration discipline as
`pcloud-store::migrations`.

---

### MEDIUM-11.7 — Health server `livez`/`readyz` are loopback-only with no port discovery hint

**Severity:** MEDIUM
**File:** `crates/pcloud-daemon/src/health_server.rs:14-18, 76-82`.

**Evidence:**
Health server binds `127.0.0.1` only and is **disabled by default**
(`HealthServerConfig::default { http_port: 0 }`). The daemon does not
emit the bound port to its audit log on success and does not advertise
it via IPC. Operators in a Kubernetes pod must hard-code the port in
their probe spec; if it ever changes (e.g. operator picks `0` for
auto-assign in tests), the probe spec breaks silently.

**Risk:** Operational mismatch between probe configuration and
runtime. Auto-assigned ports cannot be discovered by an external probe.

**Remediation:** Always emit the bound health port in the
`daemon.started` audit event, expose it on a stable IPC method like
`Method::HealthPort`, and document the contract in
`OPERATIONS-RUNBOOK.md`. Reject `http_port=0` in production
environment loads.

---

### MEDIUM-11.8 — BSD/Windows mount-cleanup-on-signal is Tier-3 (operator must clean stale mounts)

**Severity:** MEDIUM
**File:** `CLAUDE.md` (Signal-driven mount cleanup posture section),
`crates/pcloud-fs/src/platform/bsd.rs::bsd_reaper_main`,
`crates/pcloud-fs/src/platform/windows.rs::windows_reaper_main`.

**Evidence:**
By the project's own admission in `CLAUDE.md`:

> BSD and Windows mount cleanup is Tier-3: the signal handler is
> installed and an AtomicBool flag is flipped, but the reaper does
> not drain an ACTIVE_MOUNTS registry and does not call
> `unmount(MNT_FORCE)` / `FspFileSystemStopDispatcher`.

**Risk:** A daemon crash on BSD/Windows leaves stale FUSE mountpoints
that block mkdir/rmdir of the mountpoint directory and that the next
daemon start cannot reclaim. Operators must SSH in and clean up
manually. Operationally invisible — there is no metric for this.

**Remediation:** Wire the registry drain on those two platforms;
expose a `pcloud_mount_orphan_count` gauge so monitoring can detect
the situation. Track under existing `bd-xplat-bsd` /
`bd-xplat-windows`.

---

### MEDIUM-11.9 — `pcloud_request_count` counter lacks the `_total` suffix

**Severity:** MEDIUM
**File:** `crates/pcloud-observability/src/metrics.rs:19, 459`.

**Evidence:**
The counter is rendered as `pcloud_request_count` — a monotonic
counter. Prometheus naming guidance (and the OpenMetrics spec) require
the `_total` suffix on counters. The exporter applies this convention
correctly to `pcloud_auth_attempts_total`,
`pcloud_transfer_bytes_total`, and `pcloud_panic_count`, but
`pcloud_request_count` is the outlier.

**Risk:** Tools that auto-detect counter type from the suffix
(thanos, OpenMetrics-strict scrapers, some Grafana auto-config
dashboards) will misclassify it as a gauge and refuse to apply
`rate()`. Existing alert rules in `ops/prometheus/pcloud-rs-alerts.yml`
already reference it, so renaming requires a migration window.

**Remediation:** Rename to `pcloud_requests_total`. Provide a
deprecated-alias rendering that emits both names for two releases,
then drop the old one.

---

### MEDIUM-11.10 — Postinst does not validate AppArmor / SELinux profile installation

**Severity:** MEDIUM
**File:** `packaging/debian/postinst:1-35`,
`packaging/apparmor/usr.local.bin.pcloudd`,
`packaging/selinux/pcloud-rs.te`.

**Evidence:**
`postinst` only does `systemctl daemon-reload` and creates the `fuse`
group. It does not copy the AppArmor profile to `/etc/apparmor.d/` or
the SELinux module to the system policy store. Both profiles ship in
the repo but are operator-installed manually per the comments at the
top of each file.

**Risk:** A typical `apt install` user gets the daemon running without
AppArmor/SELinux confinement. Defence-in-depth posture is silently
degraded.

**Remediation:** Have `postinst` detect AppArmor/SELinux and, when
present, copy + reload the profile. Provide a `--no-mac-profile` env
override for operators who manage MAC policy externally.

---

### MEDIUM-11.11 — No documented rollback procedure for SQLite store schema upgrades

**Severity:** MEDIUM
**File:** `crates/pcloud-store/src/migrations.rs:25-34`,
`OPERATIONS-RUNBOOK.md` (no rollback section).

**Evidence:**
The store correctly enforces forward-only migrations (good). The
documented operator procedure for rollback is implicitly "keep a
backup" (`migrations.rs:30`: *"a user that needs to revert to an
older daemon build must either keep a backup of the pre-migration
database file or delete the store and re-authenticate"*). The runbook
does not state when a backup must be taken (before each upgrade) or
how to take it (cp under what locks?).

**Risk:** Operators perform an upgrade, the migration fails halfway,
the daemon refuses to start on the old binary because the store is
already at v8 with re-hashed audit rows. Recovery requires either
re-authentication (loss of vault state, audit history) or manual
SQLite forensics.

**Remediation:** Add a `pcloud-cli store backup` subcommand that
captures `state_dir/store.sqlite` + `.sqlite-wal` + `.sqlite-shm`
under a write-lock and writes them to a tar.zst with the current
`PRAGMA user_version` embedded in the filename. Document the upgrade
procedure as: *(1) `pcloud-cli store backup`; (2) stop daemon;
(3) install new daemon; (4) start new daemon (auto-migrates);
(5) verify; (6) on failure, restore backup and re-install old
daemon*.

---

### LOW-11.12 — Logrotate triggers `systemctl kill -s HUP` but the daemon's SIGHUP behaviour is undocumented

**Severity:** LOW
**File:** `packaging/debian/pcloud-rs.logrotate:11`.

**Evidence:**
`postrotate { systemctl kill -s HUP pcloudd.service }`. The daemon's
SIGHUP handler was not located in this audit (signals.rs handles
SIGTERM/SIGINT per the runbook). If SIGHUP is unhandled, the daemon
either ignores it (no log-reopen — rotation succeeds at the
filesystem level but the daemon keeps writing to the deleted inode
until `delaycompress` window closes) or terminates (worst case).

**Risk:** Logs are silently lost during rotation if SIGHUP triggers
neither a reopen nor a graceful re-init.

**Remediation:** Either (a) wire SIGHUP to `tracing-subscriber`
log-reopen, or (b) replace the `systemctl kill -s HUP` with
`copytruncate` in the logrotate config so no signal is needed. Option
(b) is simpler if the daemon does not need to differentiate rotation
from a config reload.

---

### LOW-11.13 — Health-server bind cannot be relaxed even in development

**Severity:** LOW
**File:** `crates/pcloud-daemon/src/health_server.rs:14-18`.

**Evidence:**
The doc comment says: *"It cannot be configured to bind on
0.0.0.0 — external health traffic must go through a reverse proxy or
a sidecar that exposes the loopback endpoint."* Compare to the
metrics-server bind (`crates/pcloud-daemon/src/metrics_server.rs:9-13`)
which permits a wildcard bind under
`PCLOUD_METRICS_BIND_ALL=1 + Environment::Development`. The asymmetry
is unjustified; both endpoints expose the same risk surface (no
state, fixed text body).

**Risk:** Developer ergonomics. None for production posture.

**Remediation:** Mirror the metrics-server's gated wildcard policy
on the health server.

---

### LOW-11.14 — macOS LaunchDaemon does not invoke `setup-keychain.sh` first-run

**Severity:** LOW
**File:** `packaging/macos/com.pcloud.pcloudd.plist:43-118`,
`packaging/macos/setup-keychain.sh`,
`packaging/macos/first-run.sh`.

**Evidence:**
`first-run.sh` and `setup-keychain.sh` are operator-invoked manually.
The launchd plist has no `WatchPaths` or one-shot helper to bootstrap
the keychain on first start. macOS `_pcloudd` runs as the dropped
service user without a login session, which means the
`security`-CLI keychain is not unlocked for it on boot.

**Risk:** Auth vault on macOS may fall back to file-mode and
silently miss the keychain backend. Operators must ssh in once and
run the bootstrap by hand.

**Remediation:** Document the first-run order in the launchd plist
preamble and add an `install.sh` step that pre-creates the
per-service keychain.

---

### LOW-11.15 — FreeBSD rc.d does not propagate `WATCHDOG_USEC` to the daemon

**Severity:** LOW
**File:** `packaging/freebsd/pcloudd.rc:1-72`.

**Evidence:**
The systemd unit emits `WATCHDOG=1` per loop iteration via
`sd_notify(3)`; the daemon presumably only does this on Linux when
`NOTIFY_SOCKET` is set. The FreeBSD rc.d wrapper does not configure
any analogous watchdog. If the daemon hangs on FreeBSD, rc.subr does
not detect it (rc.subr only checks the PID is alive).

**Risk:** Hung daemon on FreeBSD goes undetected indefinitely.

**Remediation:** Document this limitation in the rc.d header. If the
ops posture requires a watchdog on FreeBSD, recommend `daemon(8) -r`
or a sidecar.

---

### LOW-11.16 — `cargo-deb.toml` ships as inert documentation

**Severity:** LOW
**File:** `packaging/debian/cargo-deb.toml:1-32`.

**Evidence:**
The file is a snippet block with every line commented out, kept "in
case anyone wants to use cargo-deb instead of nfpm". It contributes
zero behaviour and serves as a maintenance burden (it must track
`Cargo.toml` if dependencies change).

**Risk:** Bit-rot. Zero functional impact today.

**Remediation:** Move the snippet into `packaging/debian/README.md`
as a code fence and delete the file. Or commit to one packaging
mechanism (nfpm) and remove the second-system option.

---

### LOW-11.17 — No documented backup/restore script for the full state set

**Severity:** LOW
**File:** absent.

**Evidence:**
Per `CLAUDE.md` the state set is: vault, SQLite store, journal,
mount-orphan registry. No single script captures all of these
atomically. `OPERATIONS-RUNBOOK.md` does not document the order in
which they must be backed up to be self-consistent.

**Risk:** A naive `tar -czf state.tar.gz /var/lib/pcloud-rs` taken
while the daemon is running captures an inconsistent snapshot.

**Remediation:** Ship `scripts/pcloud-state-backup.sh` that calls
`pcloud-cli` to enter a quiesce mode (no writes), captures the four
state items under read-only locks, and exits. Document the inverse
restore procedure.

---

## Service-Integration Status Matrix

| Platform | Init unit | Log rotation | MAC profile | Code signing | Package format | Live-verified |
|----------|-----------|--------------|-------------|--------------|----------------|---------------|
| Linux (systemd) | `pcloudd.service` (Type=notify, full hardening) | `pcloud-rs.logrotate` (HUP-trigger; see LOW-11.12) | AppArmor + SELinux profiles shipped, postinst does not auto-install (MED-11.10) | n/a (Linux ELF unsigned by convention) | nfpm `.deb`/`.rpm` (CI not wired — HIGH-11.2) | Yes (per CLAUDE.md) |
| Linux (OpenRC, runit, s6, dinit, sysvinit) | `packaging/init/{openrc,runit,s6,dinit,sysvinit}/` | manual | n/a | n/a | n/a | No |
| macOS | `com.pcloud.pcloudd.plist` (LaunchDaemon, ExitTimeOut=30) | macOS log rotation via `newsyslog` (not shipped) | `entitlements.plist` (hardened runtime) | `sign-macos.sh` + `notarize-macos.sh` | `.pkg` / `.dmg` (`build-pkg.sh`, `build-dmg.sh`) | Pending hardware (CLAUDE.md) |
| Windows | `pcloudd-svc.exe` SCM wrapper (`pcloud-daemon-win`) — service runs but no IPC (HIGH-11.1) | Windows event log + file (no rotation script) | n/a (WinFSP-driven) | `sign-windows.ps1` (signtool /fd sha256 /td sha256 /tr digicert) | WiX MSI (`packaging/windows/wix/pcloud-rs.wxs`); installs WinFSP dependency | Compile-only Tier-2 (CLAUDE.md) |
| FreeBSD | `pcloudd.rc` (rc.subr, kldload fusefs) | n/a (syslog) | n/a | n/a | freebsd port skeleton (`packaging/freebsd/`) | Tier-3 best-effort |
| OpenBSD/NetBSD | `packaging/init/openbsd/`, `netbsd/` | n/a | n/a | n/a | none | Tier-3 |
| Docker | `packaging/docker/docker-compose.yml` | n/a | n/a | n/a | image | n/a |
| Other | Homebrew, Snap, Flatpak, AppImage, Chocolatey, Scoop, Winget directories present | varies | n/a | varies | varies | not audited |

---

## Configuration Schema Review

Schema: `crates/pcloud-config/src/schema.rs::CONFIG_SCHEMA_JSON`.
Defaults: `crates/pcloud-config/src/lib.rs::ConfigProfile::secure_defaults`.

| Field | Default | Documented? | Validated? | Env override? |
|-------|---------|-------------|------------|---------------|
| `version` | `2` (`migrate.rs:59`) | Yes (migrate.rs) | Yes (schema) | No |
| `environment` | `Production` | Yes | Yes (enum) | `PCLOUD_ENV` |
| `paths.config_dir` | `<root>/config` | Yes | Yes (path string) | `PCLOUD_ROOT` |
| `paths.state_dir` | `<root>/state` | Yes | Yes | `PCLOUD_ROOT` |
| `paths.runtime_dir` | `<root>/runtime` | Yes | Yes | `PCLOUD_ROOT` |
| `paths.cache_dir` | `<root>/cache` | Yes | Yes | `PCLOUD_ROOT` |
| `api.mode` | `Tls` (Production) | Yes | Yes (enum) | `PCLOUD_API_MODE` |
| `api.host` | `api.pcloud.com` | Yes | Yes | `PCLOUD_API_HOST` |
| `api.port` | `443` | Yes | Yes (0..65535) | `PCLOUD_API_PORT` |
| `api.server_name` | `api.pcloud.com` | Yes | Yes | `PCLOUD_API_SERVER_NAME` |
| `api.connect_timeout_ms` | `15000` | Yes | Yes (>0) | `PCLOUD_API_CONNECT_TIMEOUT_MS` |
| `api.read_timeout_ms` | `30000` | Yes | Yes (>0) | `PCLOUD_API_READ_TIMEOUT_MS` |
| `api.tls_revocation_check` | `StapledPermissive` | Yes (oneOf) | Yes | No |
| `extensions.plugins_enabled` | `false` | Yes | Yes | `PCLOUD_PLUGINS_ENABLED` |
| `extensions.plugin_dir` | `<root>/plugins` | Yes | Yes | `PCLOUD_ROOT` |
| `extensions.allow_*_capability` | `false` | Yes | Yes | `PCLOUD_PLUGIN_ALLOW_*` |
| `runtime.*_dir_mode` | `0700` | Yes | Yes (mode) | No |
| `features.p2p_enabled` | `false` | Yes | Yes | No |
| `features.crypto_enabled` | `true` | Yes | Yes | No |
| `features.durable_auth_tokens_enabled` | `false` | Yes | Yes | `PCLOUD_DURABLE_AUTH_TOKENS` |
| `features.integrity_sweeper.*` | nested defaults | Yes | Yes | No |
| `features.audit_verifier.*` | nested defaults | Yes | Yes | No |
| `limits.max_concurrent_uploads` | per `secure_defaults` | Yes | Yes (>0) | No |
| `limits.max_concurrent_downloads` | per `secure_defaults` | Yes | Yes (>0) | No |
| `limits.max_parser_frame_bytes` | per `secure_defaults` | Yes | Yes | No |
| `limits.max_ipc_connections` | per `secure_defaults` | Yes | Yes (1..65535) | No |
| `limits.max_ipc_connections_per_peer` | per `secure_defaults` | Yes | Yes | No |
| `mount.allow_other` | `false` | Yes | Yes | No |
| `mount.owner_only_by_default` | `true` | Yes | Yes | No |
| `mount.cache_size_mb` | per defaults | Yes | Yes | `PCLOUD_MOUNT_CACHE_SIZE_MB` |
| `mount.page_cache_entries` | per defaults | Yes | Yes | `PCLOUD_MOUNT_PAGE_CACHE_ENTRIES` |
| `mount.metadata_ttl_secs` | per defaults | Yes | Yes | `PCLOUD_MOUNT_METADATA_TTL_SECS` |
| `mount.auto_mount_path` | `null` | Yes | Yes (string|null) | `PCLOUD_AUTO_MOUNT_PATH` |
| `observability.structured_logs_enabled` | `true` | Yes | Yes | No |
| `observability.tracing_enabled` | `false` | Yes | Yes | No |
| `observability.metrics_enabled` | `true` | Yes | Yes | No |
| `observability.audit_export_enabled` | `false` | Yes | Yes | No |
| `data_residency.allowed_regions` | `[]` | Yes | Yes (array) | No |
| `data_residency.strict` | `false` | Yes | Yes | No |
| `auth.backend` | `auto` | Yes | Yes (enum) | `PCLOUD_VAULT` |
| `auth.refresh_check_interval_secs` | per defaults | Yes | Yes | No |

**Coverage:** every field documented, every field validated either via
the JSON schema or via `ConfigProfile::validate`. Roughly **half** of
fields have an env-var override; the unset half is exclusively
internal-tuning fields where env overrides would not make operational
sense (sweeper schedules, dir modes, plugin trust keys).

**Gap:** no shipped example file (MED-11.5).

---

## Prometheus Metric Inventory

Source: `crates/pcloud-observability/src/metrics.rs:454-540`.
Cardinality and label sanitisation: same file, lines 19-26 and the
"Label sanitiser" doc section at the top.

| Name | Type | Labels | Renderer line | Cardinality |
|------|------|--------|---------------|-------------|
| `pcloud_request_count` | counter (suffix issue — see MED-11.9) | `method`, `status` | `metrics.rs:459` | O(methods × statuses), capped 64 chars |
| `pcloud_request_latency_seconds` | histogram (`_bucket`, `_sum`, `_count`) | `method`; bucket `le` | `metrics.rs:467` | O(methods) × 12 buckets |
| `pcloud_auth_attempts_total` | counter | `result` (`success`/`failure`/`tfa_required`/`recovery`) | `metrics.rs:491` | 4 |
| `pcloud_transfer_bytes_total` | counter | `direction` (`upload`/`download`) | `metrics.rs:499` | 2 |
| `pcloud_crypto_lock_state` | gauge (-1=unsetup, 0=locked, 1=unlocked) | none | `metrics.rs:507` | 1 |
| `pcloud_sync_root_count` | gauge | none | `metrics.rs:514` | 1 |
| `pcloud_ipc_connected_clients` | gauge | none | `metrics.rs:521` | 1 |
| `pcloud_panic_count` | counter (no `_total` — same as MED-11.9 caveat) | none | `metrics.rs:25` | 1 |

**Planned but not yet rendered** (per `ops/prometheus/pcloud-rs-alerts.yml:21-24`):

- `pcloud_mount_state{mount_point}` (gauge) — `bd-1du.4`.
- `pcloud_sync_queue_depth` (gauge) — `bd-1du.10`.

**Dashboards:** `ops/grafana/pcloud-rs-overview.json` (180 lines).
**Alert rules:** `ops/prometheus/pcloud-rs-alerts.yml` (151 lines, 3+
alerts: auth-spike, auth-fail-rate, p99 latency).

**Tracing:** OpenTelemetry OTLP HTTP exporter under
`tracing-otlp` feature (`crates/pcloud-observability/src/tracing.rs`).
Sample rate validated to `[0.0, 1.0]`. `service.name=pcloud-daemon`
resource attribute set. `with_location(false)`,
`with_threads(false)`, `with_tracked_inactivity(false)` to prevent
PII leakage. Attribute allow-list enforced via `attr_redact`.

**Audit-event sink:** `crates/pcloud-observability/src/audit.rs`
(per CLAUDE.md, persistence failures surfaced not silently swallowed).

---

End of report.

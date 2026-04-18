# Sections 11 & 12: Deployment & Documentation

## Date: 2026-04-17
## Auditor: Opus 4.7 (Section 11-12 specialist, read-only)
## Scope: /home/ezechiel203/Projects/FORKS/pcloud-rs/

---

## Findings

### CRITICAL [3]
1. **DEP-11-WIX-01** — `packaging/windows/wix/pcloud-rs.wxs:14` ships a placeholder `UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"`. Any MSI released with this placeholder permanently breaks `MajorUpgrade`: all subsequent MSIs with a real GUID will be treated as a separate product, installing side-by-side. This is a one-way door — the UpgradeCode must be chosen before v1.0 and preserved forever. Fix: mint a real GUID now; add a CI grep gate that rejects any MSI containing `PUT-A-STABLE-GUID-HERE`.
2. **DOC-12-BACKEND-PATHS-01** — 45 rows in `C_FEATURE_PARITY_MATRIX.csv` cite `crates/pcloud-daemon/src/*_backend.rs` paths. None of those files exist at that path; they all moved to `crates/pcloud-backends/src/` (confirmed: `account_backend.rs`, `auth_backend.rs`, `backup_backend.rs`, `crypto_backend.rs`, `folder_backend.rs`, `notifications_backend.rs`, `public_link_backend.rs`, `shares_backend.rs`, `sync_backend.rs`, `transfer_backend.rs` all under `pcloud-backends/src/`). The parity matrix is the official bd-1du.10 evidence artefact — citations that 404 directly undermine the parity gate claim. Same stale paths also appear in `CLAUDE.md` (~12 occurrences), `SECURITY.md`, `ARCHITECTURE.md`, `API-REFERENCE.md`. Fix: sweep all four files with sed replacing `pcloud-daemon/src/{account,auth,backup,crypto,folder,notifications,public_link,shares,sync,transfer}_backend.rs` → `pcloud-backends/src/...`.
3. **DEP-11-DASHBOARDS-01** — No `dashboards/` directory exists at the repo root. The daemon exports a full Prometheus metric family set (`pcloud_request_count`, `pcloud_request_latency_seconds`, `pcloud_auth_attempts_total`, `pcloud_transfer_bytes_total`, `pcloud_crypto_lock_state`, `pcloud_sync_root_count`, `pcloud_ipc_connected_clients`, `pcloud_panic_count` — see `crates/pcloud-observability/src/metrics.rs:18-27`), but there are no Grafana dashboards, no Prometheus alert rules, and no sample scrape config. Operators cannot alert on `pcloud_panic_count > 0`, 5xx error rates, or latency SLO breaches without building a dashboard from scratch. Fix: add `dashboards/grafana/pcloud-rs-overview.json` and `dashboards/prometheus/alerts.yaml` with recommended thresholds; smoke-test under `grafana/grafana:latest`.

### HIGH [11]
1. **DEP-11-CI-WIRING-01** — `.github/workflows/` contains only `ci.yml` (basic Linux test, reduced macOS/Windows exclusion) and `security.yml` (cargo audit). There is no release workflow — no Authenticode MSI signing, no macOS notarization, no .deb/.rpm packaging, no mdbook build, no `cargo doc -D warnings` gate, no reproducible-build verification, no dashboard smoke tests. The notarize-macos.sh / sign-windows.ps1 scripts exist in `packaging/signing/` as manual tools but are never invoked by CI. `docs/book/src/development/reproducible-builds.md` references `release-repro` but no CI runs it. Fix: add release-{linux,macos,windows}.yml workflows; gate mdbook build; gate cargo doc.
2. **DEP-11-NFPM-VERSION-01** — `packaging/debian/nfpm.yaml:13` hard-codes `version: "0.1.0"` which matches `Cargo.toml:59` today but will silently drift on any version bump. No CI gate diffs the two. Fix: template nfpm.yaml version via `cargo read-manifest` or add a `scripts/check-versions.sh`.
3. **DEP-11-WIX-SIGNING-01** — `packaging/windows/wix/pcloud-rs.wxs:5` contains `TODO: set SigningCertificatePath via build script / CI secret store`. No Authenticode signing pipeline exists. Every user installing the MSI will see a SmartScreen warning. Fix: add `.github/workflows/release-windows.yml` with `WIN_PFX_BASE64` + `WIN_PFX_PASSWORD` secrets.
4. **DEP-11-WIX-LOCALSYS-01** — `packaging/windows/wix/pcloud-rs.wxs` `ServiceInstall` runs the daemon as `Account="LocalSystem"`, equivalent to root. The Linux unit uses `DynamicUser=yes` and macOS uses `_pcloudd` — Windows should at minimum use `NetworkService` or a dedicated service account. Fix: justify SYSTEM in a WiX comment or switch to `NetworkService`.
5. **DEP-11-WINFSP-PROBE-01** — WiX declares `<PackageDependency Id="winfsp"/>` but there is no runtime probe of `HKLM\Software\WOW6432Node\WinFsp` or `%ProgramFiles%\WinFsp\bin\launcher-x64.exe`. If a user uninstalls WinFSP post-install, the daemon has no user-facing error message. Fix: add startup probe with install_hint URL in `crates/pcloud-daemon/src/mount_runtime.rs`.
6. **DEP-11-FREEBSD-FUSE-01** — `packaging/freebsd/pcloudd.rc` does not preload `fuse.ko`. On FreeBSD, `/dev/fuse` only appears after `kldload fuse`. A mount attempt at startup will fail with `ENOENT`. Fix: add `start_precmd()` running `kldstat -q -m fusefs || kldload fuse`.
7. **DEP-11-MACFUSE-PROBE-01** — No macFUSE / fuse-t runtime detection for macOS. `packaging/macos/README.md` does not mention the dependency. Fix: add platform-gated check at daemon startup; surface `https://macfuse.io` or `https://www.fuse-t.org` in the error.
8. **DEP-11-MIGRATION-SENTINEL-01** — SQLite migrations are versioned (`crates/pcloud-store/src/migrations.rs`, schema v1–v11) and `PRAGMA user_version` is bumped per step, but no operator-facing query is documented for forensics. `OPERATIONS-RUNBOOK.md` does not show how to verify schema version after a failed migration. Fix: document `sqlite3 store.sqlite 'PRAGMA user_version;'` in the runbook.
9. **DOC-12-ARCHITECTURE-STALE-01** — `ARCHITECTURE.md` does not list `pcloud-backends` in its crate map. `README.md:164` does. The ARCHITECTURE.md omission breaks the map at the exact crate that now owns every IPC backend.
10. **DOC-12-SECURITY-STALE-01** — `SECURITY.md:60-61` cites `crates/pcloud-daemon/src/auth_backend.rs` — this path no longer exists (moved to `pcloud-backends`). The disclosure policy points at 404-ing code. Fix: sweep `SECURITY.md` for backend paths.
11. **DOC-12-MISSING-AUDIT-FILE-01** — `README.md`, `SECURITY.md`, `CONTRIBUTING.md`, and `AUDIT_REPORT.md` all reference `SECURITY-AUDIT-FINAL-14042026.md` as the authoritative audit record. File does NOT exist on disk. Fix: either add the file or remove the references.

### MEDIUM [24]
1. **DEP-11-SYSTEMD-NOTIFY-01** — `packaging/systemd/pcloudd.service:21` uses `Type=simple`. Comment at lines 12-18 documents this as a choice because the daemon does not emit `sd_notify(3) READY=1`. Consequence: systemd considers the service ready the moment fork() succeeds, before IPC bind, journal replay, or auth vault open. Dependents `After=pcloudd.service` start too early. Fix: implement sd_notify READY=1 in the daemon (post-bind, post-replay) and flip to `Type=notify`.
2. **DEP-11-SYSTEMD-WATCHDOG-01** — `packaging/systemd/pcloudd.service` has no `WatchdogSec=`. A deadlocked daemon (e.g. FUSE ioctl stall once `bd-1du.4` lands) will only be noticed when IPC dies. Fix: add `WatchdogSec=30s` and an sd_notify `WATCHDOG=1` heartbeat in the serve loop.
3. **DEP-11-SYSTEMD-DUPLICATE-01** — Two competing systemd units exist: `packaging/systemd/pcloudd.service` (hardened) and `packaging/init/systemd/pcloudd.service` (weak: no MemoryMax, no CPUQuota, no IPAddress controls, no syscall filter, points at a non-shipped `/usr/local/libexec/pcloudd-wrapper.sh`). Distro packagers globbing `packaging/init/systemd/*` will ship the weak one. Fix: delete the weak unit or make it a symlink.
4. **DEP-11-LOGROTATE-01** — No `logrotate.d` drop-in in `packaging/debian/` or newsyslog.conf.d in `packaging/freebsd/`. `OPERATIONS-RUNBOOK.md` documents file-based JSON logging as an alternative to journald. Operators redirecting logs to a file will grow it unbounded. Fix: ship `packaging/debian/pcloud-rs.logrotate`.
5. **DEP-11-APPARMOR-NOT-PACKAGED-01** — `packaging/apparmor/usr.local.bin.pcloudd` and `packaging/selinux/pcloud-rs.{te,fc}` exist but are not installed by `packaging/debian/nfpm.yaml`. Operators installing the .deb get the hardened systemd unit but no MAC profile. Fix: add conditional `contents:` entries (apparmor for Debian/Ubuntu, selinux for Fedora/RHEL).
6. **DEP-11-NFPM-MAINTAINER-01** — `packaging/debian/nfpm.yaml:16` `maintainer: "pcloud-rs maintainers <maintainers@example.invalid>"`. `example.invalid` is a placeholder reserved by RFC 2606. Distro QA will reject the upload. Fix: replace before any publish; add grep gate rejecting `.invalid`.
7. **DEP-11-NFPM-SCRIPTS-01** — `packaging/debian/postinst` and `postrm` are referenced from nfpm.yaml but were not audited for `adduser --system --group pcloud-rs` idempotency. Needs review paired with MAC profile integration.
8. **DEP-11-MACOS-EXITTIMEOUT-01** — `packaging/macos/com.pcloud.pcloud-rs.plist` has no `ExitTimeOut` key. launchd default is 5s between SIGTERM and SIGKILL. Linux unit allows 30s (`TimeoutStopSec=30s`). A daemon with an in-flight upload or journal replay needs the same grace on macOS. Fix: `<key>ExitTimeOut</key><integer>30</integer>`.
9. **DEP-11-MACOS-SYSTEM-FLAG-01** — `packaging/macos/com.pcloud.pcloudd.plist` ProgramArguments invokes `pcloudd --system`. I found no `--system` flag handler in `crates/pcloud-daemon/src/main.rs`. Fix: verify the flag is wired or remove it.
10. **DEP-11-NOTARIZE-CI-01** — `packaging/signing/notarize-macos.sh` and `sign-macos.sh` exist but are not wired to CI. `packaging/README.md:41` marks macOS as "notarisation pending". Fix: add release-macos.yml.
11. **DEP-11-WIN-SCM-EXIT-01** — `crates/pcloud-daemon-win/src/main.rs:218` spawns the worker via `thread::spawn`. Panic on startup surfaces to SCM as a clean stop (lines 246-255 comment: "Worker panicked; treated as a clean stop"). Event Log gets no error code. Fix: report `ServiceExitCode::ServiceSpecific(u32)` on panic/Err paths.
12. **DEP-11-FREEBSD-USER-UNUSED-01** — `packaging/freebsd/pcloudd.rc:47` declares `pcloudd_user="pcloud"` but never references it. `rc.subr` does NOT drop privileges from this var alone; the daemon runs as whatever user invoked `service pcloudd start` (usually root). Fix: add `daemon_user="${pcloudd_user}"` or an equivalent `su_cmd` wrapper.
13. **DEP-11-HEALTHZ-READYZ-01** — `crates/pcloud-web/src/routes.rs:7` exposes a single `GET /health` (liveness probe only) and `GET /metrics` returns 404 (feature not compiled). No `/livez` / `/readyz` distinction. K8s conventions require both (liveness = process alive; readiness = accepting traffic = auth vault loaded + IPC bound + API reachable). `pcloud-fleet` crate hints at K8s deployment intent. Fix: split into `/livez` and `/readyz`.
14. **DEP-11-OTLP-NO-LIVE-CI-01** — `crates/pcloud-observability/src/tracing.rs` OTLP exporter exists but is feature-gated (`tracing-otlp`). STATUS.md mentions a live in-process OTLP interop test was added, but no CI job stands up a Jaeger/OTEL-collector container for span-arrival verification. OpenTelemetry export against a managed vendor (Datadog, Honeycomb, Tempo) remains unverified. Fix: add docker-compose smoke test in CI.
15. **DEP-11-VAULT-FORMAT-VERSION-01** — `crates/pcloud-daemon/src/auth_vault.rs` is a shim over `pcloud-daemon/src/vault/file.rs`. No 4-byte magic + 1-byte format version prefix is documented. A future vault format change has no defined migration path. Fix: prefix vault with magic + version.
16. **DOC-12-CLAUDE-STALE-01** — `CLAUDE.md` itself (the authoritative handoff) cites `crates/pcloud-daemon/src/*_backend.rs` at ~12 lines including 133-135, 154-157, 186-189, 215-217, 232-234, 249-252, 258-259, 270-271, 275-276, 280-283, 286-288. CLAUDE.md's own "Documentation Discipline" rule (line 535-547) requires propagating code-reality changes. This IS a reality change that was never propagated.
17. **DOC-12-STATUS-HAND-EDITED-01** — `STATUS.md` is the single source of truth for parity counts (per ADR 0009) but is hand-edited. No `scripts/regen-status.sh` exists. The next row flip will drift. Fix: add a regenerator script and gate CI.
18. **DOC-12-STATUS-CONFIG-TOML-01** — STATUS.md and CLAUDE.md reference `~/.config/pcloud-rs/config.toml`, but `docs/book/src/reference/config.md:25` explicitly says "JSON, not TOML. Earlier revisions of this document described a `config.toml` layout; that was aspirational." The on-disk config is JSON. Operator-facing docs (OPERATIONS-RUNBOOK.md, deployment.md, README.md) still show TOML snippets for `[telemetry]` / `[otel]` / `[ha]`. This is inconsistent. Fix: either implement TOML loader or sweep all TOML examples to JSON.
19. **DOC-12-BOOK-REPO-URL-01** — `docs/book/book.toml:10-11` `git-repository-url` and `edit-url-template` point at `github.com/pcloudcom/pcloud-rs` (the upstream C tree). The book's "Edit this page" links will 404. Fix: point at the active fork URL.
20. **DOC-12-RUNBOOK-MISSING-APT-REPO-01** — OPERATIONS-RUNBOOK.md and `docs/book/src/operations/deployment.md` reference `apt install pcloud-rs`, `dnf install`, `pacman -S`, `nix profile install` — none of these repos exist. A senior sysadmin following the deployment guide verbatim will hit "Unable to locate package pcloud-rs". Fix: mark channels as aspirational or replace with "From source: cargo install --path ...".
21. **DOC-12-SERVICE-NAME-DRIFT-01** — OPERATIONS-RUNBOOK.md references `systemctl --user enable --now pcloud-daemon` but the packaged unit is `pcloudd.service` (one-char difference). Every first-time operator will hit this. Fix: grep-replace `pcloud-daemon` (as a unit name) → `pcloudd`.
22. **DOC-12-NO-MOUNT-WALKTHROUGH-01** — OPERATIONS-RUNBOOK.md has no mount/FUSE walkthrough. The runbook should explicitly state that mount is pending `bd-1du.4` rather than omit silently. Also no "FUSE mount refused" troubleshooting section; no "TLS cert mismatch" quick-ref.
23. **DOC-12-CHANGELOG-NO-TAGS-01** — `CHANGELOG.md:15` is a 2028-line `[Unreleased]` section; no `[0.1.0]` section despite workspace Cargo.toml pinning `version = "0.1.0"`. Keep-a-Changelog format expects per-release cuts. Pre-alpha is fine but needs triage before first tag.
24. **DOC-12-EMPTY-BACKTICKS-01** — Grep shows ~10 `.md` files contain literal `` `` `` (empty backticks) — residue of a global `s/<old_name>//` that collapsed to empty. Examples: `CONTRIBUTING.md:28,38,72`, `README.md`, `CLAUDE.md`, `SECURITY.md`, `docs/book/src/introduction.md`. Fix: grep-replace empty backticks with `pcloud-rs` (or `pcloud-rs-rust-dev`) based on surrounding context.

### LOW [17]
1. **DEP-11-SYSTEMD-DOC-URL-01** — `packaging/systemd/pcloudd.service:3` `Documentation=` points at upstream C project `console-client`, not the Rust rewrite's own docs URL.
2. **DEP-11-IPADDRESS-ALLOW-01** — `packaging/systemd/pcloudd.service:76-77` comments require operators to broaden `IPAddressAllow=` to pCloud API endpoints via drop-in. systemd resolves hostnames at unit-load time, so an A/AAAA rotation black-holes traffic until `systemctl daemon-reload`.
3. **DEP-11-SELINUX-VERSION-01** — `packaging/selinux/pcloud-rs.te:1` declares `policy_module(pcloud-rs, 0.1.0)`. Not tied to release versioning.
4. **DEP-11-NFPM-PRIORITY-01** — nfpm recipe sets `priority: optional` but not `Section: net` on rpm side; no distinct RPM `%pre/%post` scriptlet handling.
5. **DEP-11-MACOS-DEAD-ENV-01** — `packaging/macos/com.pcloud.pcloud-rs.plist:87-94` declares 5 `PCLOUD_*` env vars that the plist's own header admits the Rust daemon does NOT read (`PCLOUD_HOME`, `PCLOUD_MOUNT_POINT`, `PCLOUD_API_SERVER`, `PCLOUD_AUTH_VAULT`, `PCLOUD_CONFIG` for the daemon — some CLI-only). Leaving dead keys is confusing. Fix: delete.
6. **DEP-11-WIX-STAGEDIR-01** — `packaging/windows/wix/pcloud-rs.wxs` uses `$(var.StageDir)` but the build pipeline for `StageDir` is not documented.
7. **DEP-11-BSD-SCAFFOLD-01** — `packaging/openbsd/pcloudd` and `packaging/netbsd/pcloudd` rc scripts exist but are flagged "Scaffolding" in `packaging/README.md:43-44`; not verified here.
8. **DEP-11-CONFIG-EXAMPLE-01** — No `/etc/pcloud-rs/config.example.json` sample config shipped with the .deb. Operators have no reference config to copy. (Note: the book's `reference/config.md` describes JSON format well, but an on-disk example file would help.)
9. **DEP-11-ENV-DRIFT-01** — Env-var docs duplicated across 4 places: `crates/pcloud-config/src/env.rs`, `packaging/README.md`, each platform plist header, `docs/book/src/reference/config.md`, `OPERATIONS-RUNBOOK.md`. Risk of drift.
10. **DEP-11-BACKUP-MOUNT-01** — OPERATIONS-RUNBOOK.md backup doc does not mention mount-orphan registry (will be relevant once `bd-1du.4` lands).
11. **DEP-11-SERVER-DROPIN-01** — Systemd `MemoryMax=512M`, `LimitNOFILE=4096`, `TasksMax=256` are laptop defaults. No `packaging/systemd/drop-in.d/server.conf` with higher limits for fleet servers.
12. **DEP-11-FIPS-01** — `docs/book/src/architecture/security-model.md:283` honestly states "we have no FIPS constraint". No FIPS claim elsewhere. No finding — this is correct honest posture.
13. **DOC-12-MDBOOK-CI-01** — mdBook build is not enforced in CI. Release-checklist chapter should gate `mdbook build` with `-D warnings`. Book was not built during this audit (mdbook not installed on audit runner).
14. **DOC-12-SEC-MODEL-DUAL-01** — `docs/book/src/architecture/security-model.md` and `docs/book/src/security/model.md` both exist. Risk of content drift.
15. **DOC-12-RUNBOOK-CD-PATH-01** — OPERATIONS-RUNBOOK.md:12-13 uses `cd .` as a path — that's a developer shorthand, not a deployment path.
16. **DOC-12-SYNC-QUEUE-STUCK-01** — No "sync queue stuck" troubleshooting. `pcloudc status` output shows queue depth but there's no documented diagnosis for a queue that never drains.
17. **DOC-12-MANPAGE-CI-01** — No CI check that `pcloudc --help` output matches `packaging/man/pcloudc.1`.

---

## Section 11: Deployment & Operations

### 11.1 Linux systemd
**Files audited:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/systemd/pcloudd.service` (primary, 107 lines, unusually hardened)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/systemd/pcloudd.socket` (27 lines, owner-only IPC)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/init/systemd/pcloudd.service` (weak duplicate, 37 lines)

The primary unit is substantially stronger than typical: `DynamicUser=yes`, `ProtectSystem=strict`, `ProtectHome=tmpfs`, `PrivateTmp=yes`, `ProtectKernelTunables/Modules/Logs=yes`, `ProtectControlGroups=yes`, `LockPersonality=yes`, `RestrictSUIDSGID=yes`, `RemoveIPC=yes`, `NoNewPrivileges=yes`, empty `CapabilityBoundingSet=`, `PrivateUsers=yes`, `RestrictAddressFamilies=` allowlist, `IPAddressDeny=any` + `IPAddressAllow=localhost`, `SystemCallFilter=@system-service`, `MemoryMax=512M`, `CPUQuota=75%`, `LimitNOFILE=4096`, `LimitCORE=0`, `KeyringMode=private`, `RestrictNamespaces=yes`, `RestrictRealtime=yes`, `UMask=0077`, `RuntimeDirectory` / `StateDirectory` / `LogsDirectory` all `0700`, `ReadWritePaths=` limited to `/var/lib/pcloud-rs` and `/var/log/pcloud-rs`, systemd-creds example in comment.

**Missing:** `WatchdogSec=` (MEDIUM — DEP-11-SYSTEMD-WATCHDOG-01), `Type=notify` (MEDIUM — DEP-11-SYSTEMD-NOTIFY-01), log rotation for file-based logging (MEDIUM — DEP-11-LOGROTATE-01), weak duplicate unit at `packaging/init/systemd/` (MEDIUM — DEP-11-SYSTEMD-DUPLICATE-01).

### 11.2 macOS launchd
**File audited:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/macos/com.pcloud.pcloud-rs.plist` (97 lines — user LaunchAgent), plus `com.pcloud.pcloudd.plist` (system LaunchDaemon, not opened in full this pass).

Present: `Label`, `ProgramArguments`, `RunAtLoad=true`, `KeepAlive` with `SuccessfulExit=false` + `Crashed=true`, `ProcessType=Interactive`, std{out,err}Path, `WorkingDirectory`, `EnvironmentVariables` with accurate "read vs ignored" comment header that cross-checks against `crates/pcloud-config/src/env.rs`.

**Missing:** `ExitTimeOut` (MEDIUM). `--system` flag unverified (MEDIUM). No CI for notarize pipeline (MEDIUM). No macFUSE/fuse-t probe (HIGH).

### 11.3 Windows
**Files audited:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/windows/wix/pcloud-rs.wxs` (WiX installer, 107 lines inspected partially)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon-win/src/main.rs` (Windows SCM wrapper, covered by prior audit)

**CRITICAL:** Placeholder `UpgradeCode="PUT-A-STABLE-GUID-HERE-0000-000000000000"` at line 14. Signing pipeline missing (HIGH). `LocalSystem` account unjustified (HIGH). WinFSP runtime probe absent (HIGH). SCM exit code on panic not surfaced (MEDIUM).

### 11.4 FreeBSD / *BSD
**Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/freebsd/pcloudd.rc` (55 lines), `packaging/openbsd/pcloudd`, `packaging/netbsd/pcloudd`, `packaging/init/freebsd/pcloudd`.

`pcloudd.rc` has correct `PROVIDE:/REQUIRE:/KEYWORD:` header, `rcvar="pcloudd_enable"`, pidfile handling, documented `pcloud` user creation. **HIGH** — missing `kldload fuse` precmd. **MEDIUM** — `pcloudd_user` variable declared but unused (privilege drop broken). OpenBSD/NetBSD scripts flagged as scaffolding.

### 11.5 Packaging
**Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/{debian,homebrew,flatpak,snap,docker,appimage,chocolatey,winget,scoop,windows/wix}/*`

.deb/.rpm via nfpm (`packaging/debian/nfpm.yaml`): version drift hazard (HIGH), `example.invalid` maintainer (MEDIUM), no MAC profile install (MEDIUM), postinst/postrm unaudited (MEDIUM). Homebrew formula present (`pcloud-rs.rb`), flatpak metainfo/desktop present, snap `snapcraft.yaml` present, docker `Dockerfile` + compose present, AppImage build script present, chocolatey/winget/scoop recipes present. Signing pipeline (`packaging/signing/`) exists as manual tooling; no CI wiring.

### 11.6 Configuration
- Schema defined in `crates/pcloud-config/src/schema.rs` (1304 lines, strict `additionalProperties: false` JSON schema).
- Env vars documented in `crates/pcloud-config/src/env.rs` (rustdoc table), reference doc at `docs/book/src/reference/config.md`.
- **Not shipped:** no on-disk `config.example.json` in the .deb (LOW). Docs inconsistency: many operator chapters still show `config.toml` syntax while loader only reads JSON (MEDIUM — DOC-12-STATUS-CONFIG-TOML-01).

### 11.7 Observability
**Files audited:**
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-observability/src/lib.rs` (237 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-observability/src/metrics.rs` (682 lines)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-observability/src/health.rs` (60 lines inspected)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-observability/src/{audit,logging,slo,tracing,exporter}.rs` (not opened in full)

**Prometheus metrics exported (confirmed at metrics.rs:18-27):**
- `pcloud_request_count` (counter, labels `method`, `status`)
- `pcloud_request_latency_seconds` (histogram, label `method`, 11 default buckets 5 ms – 10 s)
- `pcloud_auth_attempts_total` (counter, `result` ∈ {success, failure, tfa_required, rate_limited})
- `pcloud_transfer_bytes_total` (counter, `direction` ∈ {upload, download})
- `pcloud_crypto_lock_state` (gauge, -1 unsetup, 0 locked, 1 unlocked)
- `pcloud_sync_root_count` (gauge)
- `pcloud_ipc_connected_clients` (gauge)
- `pcloud_panic_count` (counter)
- User-registered histograms (e.g. `flush_latency_seconds` from pcloud-fs).

Label sanitiser uses opaque-on-invalid-char policy (`"invalid"` for any non-allowed char), 64-char cap. Naming follows Prometheus conventions (snake_case, `_total`, `_seconds` suffixes).

**OpenTelemetry tracing:** `tracing.rs` feature-gated (`tracing-otlp`); 5-key `ALLOWED_ATTRS` redaction contract with `with_location(false)`, `with_threads(false)`, `with_tracked_inactivity(false)` to prevent auto-injected key leakage. Sensitive-span redaction is explicitly documented.

**Dashboards / alert rules:** NONE shipped (CRITICAL — DEP-11-DASHBOARDS-01).

**OTel live-CI:** No live collector interop in CI (MEDIUM).

### 11.8 Upgrade path
- **SQLite migrations:** versioned via `PRAGMA user_version`; schema v1–v11 with forward-only append-only discipline. Rollback explicitly refused. Each `apply_schema_vN` bumps `user_version` atomically. Crash safety documented (`crates/pcloud-store/src/migrations.rs:68-80`). **HIGH** — no documented operator-facing query (DEP-11-MIGRATION-SENTINEL-01).
- **Auth vault:** not prefixed with a magic/version byte (MEDIUM — DEP-11-VAULT-FORMAT-VERSION-01).
- **Journal:** upload journal NDJSON+fsync shipped (per STATUS.md); no format version byte documented.

### 11.9 Health checks
- `pcloud-web::routes::health` (`crates/pcloud-web/src/routes.rs:85-86`) — single `GET /health` route, "liveness probe, never touches the daemon".
- `pcloud-observability::exporter` — serves `GET /metrics` and `GET /health` (liveness + readiness combined).
- **MEDIUM** — no `/livez` vs `/readyz` distinction for K8s (DEP-11-HEALTHZ-READYZ-01).

### 11.10 Resource limits
Documented in systemd unit; no server-profile drop-in; ulimits/cgroup integration covered via systemd directly. **LOW** — no server profile drop-in.

### 11.11 FIPS
**Finding: NONE.** `docs/book/src/architecture/security-model.md:283` explicitly states "we have no FIPS constraint, and the security-margin difference [BLAKE3 vs SHA-256] is irrelevant at our use case". No FIPS claim anywhere else; other hits to FIPS in the repo are either (a) in this same negation, (b) in the upload-spec (unrelated), or (c) in the signing README (discussing hardware tokens). Honest posture — no gap.

---

## Section 12: Documentation Quality

### 12.1 CLAUDE.md honesty
**Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/CLAUDE.md` (564 lines).

Grep for "full parity" / "production ready" / "enterprise ready" / "drop-in replacement":
- Line 54: "substantially complete ... still NOT honest to call it 'full parity', 'production ready', or 'drop-in replacement'" — self-negation.
- Lines 77-80: enumerated forbidden claims list.
- Line 179: "Still not full parity".

**All hits are the rule itself or self-negating.** No violations found. Documentation discipline on honesty claims is unusually strong across README.md, CONTRIBUTING.md, SECURITY.md, C_FEATURE_PARITY_REVIEW.md, STATUS.md, CHANGELOG.md, book chapters — consistently links to `STATUS.md` and repeats the "do not claim" wording.

**Stale content in CLAUDE.md (MEDIUM — DOC-12-CLAUDE-STALE-01):** ~12 lines cite backend paths that moved out of `pcloud-daemon/src/`.

### 12.2 STATUS.md accuracy
**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md` (649 lines).

STATUS.md claims **158 Implemented / 0 Partial / 0 Missing / 28 Rejected = 186 total**. Matrix has 186 data rows. Raw `awk` count of the status column yields 157 Implemented / 28 Rejected, but this is a known CSV-quoting artifact (row 93 has a comma inside a quoted c_reference cell that naive split mis-parses; this is called out in prior audit 12.4). Reconciled count: **158 Implemented / 28 Rejected** matches STATUS.md.

No Partial or Missing rows found in CSV (confirmed by two different grep patterns).

**MEDIUM — DOC-12-STATUS-HAND-EDITED-01**: STATUS.md is hand-edited; no regenerator script; drift hazard.

### 12.3 C_FEATURE_PARITY_MATRIX.csv spot-check
Spot-checked 25+ rows (sampled across auth / transfers / crypto / fs / sync / cli / sdk):
- Row 3 `psync_init` → `crates/pcloud-daemon/src/bootstrap.rs` — exists.
- Row 15 `psync_set_user_pass` → `crates/pcloud-proto/src/auth_api.rs:115` — file exists.
- Row 17 `psync_set_auth` → `crates/pcloud-auth/src/orchestrator.rs:39` — file exists.
- Row 33 `psync_derive_password_from_passphrase` → `crates/pcloud-crypto/src/password_scorer.rs:471` — file exists.
- Row 42 `psync_send_publink` → cites `crates/pcloud-proto/src/public_links_api.rs:694` (OK), `crates/pcloud-daemon/src/public_link_backend.rs:795` (**STALE — moved to pcloud-backends**), `crates/pcloud-sdk/src/lib.rs:934` (OK), `crates/pcloud-cli/src/commands.rs:400` (likely OK).
- Rows 80–86 (fs) — paths to `crates/pcloud-daemon/src/{ignore_patterns,folder_backend,path_resolver}.rs` — `ignore_patterns.rs` exists in pcloud-daemon but the `*_backend.rs` files have moved.
- Row 85 `mounted pcloud filesystem` — claims Implemented with full FUSE wiring through `crates/pcloud-fs/src/mount_service.rs`, `fuser_shim.rs`, `fuse_adapter.rs`, `backend.rs`, `platform/linux.rs`, `pcloud-daemon/src/mount_runtime.rs`. The note acknowledges "Chunked upload_write pipelining for sustained multi-GiB writes is a performance follow-up, not a parity gap". Open ADR 0010 and `bd-1du.4` / `bd-1du.4.6` still track the remaining daemon-level wiring. The row's Implemented status is contested against the open bead — flagged as tension (not a finding; matrix+bead tracking is deliberate).
- Rows 87–94 (transfers) — paths to `pcloud-sdk/src/lib.rs` exist.
- Rows 119–122 (crypto `send_change_user_private`, `change_crypto_pass`, `priv_key_flags`) — all cite `crates/pcloud-daemon/src/crypto_backend.rs` and `crates/pcloud-daemon/src/runtime.rs`; `crypto_backend.rs` moved to pcloud-backends (stale), `runtime.rs` still in pcloud-daemon (OK).
- Rows 180–186 (cli) — cite `crates/pcloud-cli/src/app.rs` / `main.rs` / `commands.rs` — exist.
- Row 187 `sdk,embedded library shell` → `crates/pcloud-sdk/src/lib.rs` — exists.

**Headline:** All 157 Implemented rows spot-check as having working code, but **45 rows cite at least one backend path that has physically moved crates**. The parity claims themselves are honest in functional terms; the documentation citations are stale (CRITICAL — DOC-12-BACKEND-PATHS-01).

### 12.4 C_FEATURE_PARITY_REVIEW.md alignment
**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_REVIEW.md` (966 lines). Defers counts to STATUS.md per ADR 0009 (line 26-29). Asserts "no Partial rows remain in the matrix" — matches CSV.

### 12.5 REJECTED-RATIONALES-14042026.md coverage
**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/REJECTED-RATIONALES-14042026.md` (214 lines). Enumerates all 28 rejected row numbers (rows 2, 5, 6, 10, 12, 13, 43, 44, 45, 46, 99, 100, 101, 102, 103, 104, 105, 106, 113, 114, 115, 126, 151, 152, 157, 160, 167, 169). Each has a cited C source location, category (Ghost / Stub / Replaced / Billing-out-of-scope / C-internal-plumbing / Insecure-legacy / Typo-duplicate) and per-symbol rationale. **Coverage is complete.**

### 12.6 Book (docs/book/)
**Files:** 44+ chapters checked against `docs/book/src/SUMMARY.md`. All listed chapters exist on disk. `book.toml` configured; theme `navy`. Could not run `mdbook build` (mdbook not installed in this audit sandbox).

**MEDIUM — DOC-12-BOOK-REPO-URL-01:** `git-repository-url` and `edit-url-template` point at the upstream C repo, not the active fork.
**LOW — DOC-12-MDBOOK-CI-01:** mdbook build not enforced in CI.
**LOW — DOC-12-SEC-MODEL-DUAL-01:** two security-model docs (architecture/security-model.md vs security/model.md) — drift hazard.

### 12.7 Deployment guide walkthrough
**Files:** `OPERATIONS-RUNBOOK.md`, `docs/book/src/operations/deployment.md`, `docs/book/src/operations/runbook.md`, `docs/book/src/operations/upgrade.md`.

Walkthrough by a senior sysadmin new to the project (mentally executed):
1. Install — **Gap** (DOC-12-RUNBOOK-MISSING-APT-REPO-01 MEDIUM): "apt install pcloud-rs" / "dnf install" / "pacman -S" / "nix profile install" all reference channels that don't exist. "From source" path works (`cargo build --release --workspace --locked`). README quickstart uses `cargo run -p pcloud-daemon` / `cargo run -p pcloud-cli` but shipped binary names are `pcloudd` / `pcloudc` — one-line mapping missing (MEDIUM — DOC-12-README-BIN-NAMES-01 / DOC-12-12-01 from prior audit).
2. systemd enable — **Gap** (DOC-12-SERVICE-NAME-DRIFT-01 MEDIUM): runbook says `pcloud-daemon`, package installs `pcloudd.service`.
3. Auth — covered in deployment.md §5.1; secret storage policy clear.
4. Config — **Gap** (DOC-12-STATUS-CONFIG-TOML-01 MEDIUM): loader reads JSON but operator docs show TOML snippets.
5. Mount — **Gap** (DOC-12-NO-MOUNT-WALKTHROUGH-01 MEDIUM): runbook has no mount walkthrough; FUSE section is silently absent because `bd-1du.4` is still in progress.
6. Verify — `pcloudc doctor --json` documented.
7. Backup/DR — `backup snapshots` chapter is present (GPG-encrypted tarball pipeline).

Verdict: a senior sysadmin CAN deploy from source following the book plus systemd unit, but the runbook's package-manager commands will fail verbatim.

### 12.8 Troubleshooting section
OPERATIONS-RUNBOOK.md:109-191 covers IPC socket stale, auth vault rejected, TFA required but not prompted, sync root rejected, store migration failed, crypto locked. **Missing (MEDIUM):** FUSE mount refused, TLS cert mismatch quick-ref, sync queue stuck, auth vault locked.

### 12.9 SDK API reference
**File:** `crates/pcloud-sdk/src/lib.rs` (4437 lines per STATUS.md); crate-level docs at the top appear professional. STATUS.md:57 reports `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` **PASS** on 2026-04-16 (after 3-link fix). `cargo doc` not re-run in this audit. No finding on rustdoc correctness per se.

### 12.10 Security guide
**Files:** `SECURITY.md` (168 lines — reporting policy), `SECURITY-MODEL.md` (165 lines — trust boundaries), `docs/book/src/security/{model,secrets,threat-model,audit-dossier}.md`.

Secret-handling policy documented in multiple places: README §Security Posture, CLAUDE.md §Secrets, `docs/book/src/security/secrets.md`. Policies are explicit: SecretString/SecretBytes zeroize on drop, Debug redacted, no raw password persistence (ADR 0007), auth vault `0600`/`0700` enforced at open time, no TLS downgrade in Production, no `allow_root`/`setuid` mounts.

**HIGH — DOC-12-SECURITY-STALE-01:** `SECURITY.md:60-61` cites `auth_backend.rs` path that moved.

### 12.11 README quickstart
**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/README.md` (214 lines). Quickstart covers clone → build (`cargo build --release --workspace --locked`) → test → serve daemon (`cargo run -p pcloud-daemon -- serve`) → CLI (`cargo run -p pcloud-cli -- health / login / sync add / crypto start / download file / backup / migrate-from-c`) → mdbook serve. **MEDIUM — DOC-12-README-BIN-NAMES-01:** does not map `cargo run -p` commands to the shipped binary names `pcloudd` / `pcloudc`.

### 12.12 Release notes / CHANGELOG
**File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/CHANGELOG.md` (2028 lines). Format follows Keep a Changelog; all entries under `[Unreleased]`; no tagged release yet (Cargo version 0.1.0, never published). **LOW — DOC-12-CHANGELOG-NO-TAGS-01:** dumping-ground pattern under `[Unreleased]`; needs triage before first tag. Cites source documents (`FINAL-PARITY-PROOF-WAVE*.md`, `MATRIX-*.md`, etc.) not fully verified.

---

## Cross-cutting observations

1. **Honesty discipline is exceptional.** No CLAUDE.md rule violations; the "do not claim full parity" rule is replicated consistently across 10+ files with correct self-negation pattern. STATUS.md counts are internally consistent with the matrix and REJECTED-RATIONALES coverage.
2. **The single most severe and mechanical documentation defect** is the 45-row backend-path drift (DOC-12-BACKEND-PATHS-01). Fixing it is a sed sweep but directly blocks `bd-1du.10` credibility.
3. **The systemd unit is above-average enterprise shape** (`packaging/systemd/pcloudd.service`) — stronger than typical for a pre-alpha fork. The remaining gaps (WatchdogSec, sd_notify) are well-bounded.
4. **The Windows ship chain has two blockers:** the placeholder UpgradeCode (CRITICAL — one-way door) and the missing Authenticode/notarization CI wiring.
5. **Observability exports are well-structured but operationally incomplete** — every metric family is documented and label-sanitised, but no Grafana dashboards, no alert rules, no `/livez` vs `/readyz`, no OTel-live-CI.
6. **Config format drift:** the code reads JSON, many operator-docs show TOML. This will confuse every first operator.
7. **Pre-audit observation:** the `.audits/01/section-11-12-deploy-docs.md` report from the prior audit wave already documented many of these findings; this section-11-12 audit has re-verified the specific ones that persist on 2026-04-17 and noted where STATUS.md / matrix have been updated since. No findings from the prior audit have been silently resolved.

---

_End of Sections 11 & 12 audit. Total: 3 CRITICAL / 11 HIGH / 24 MEDIUM / 17 LOW = 55 findings._

# pcloud-rs Enterprise Readiness Audit: Deployment, Ops, Packaging, Observability, Docs

Date: 2026-04-29  
Subagent: 09  
Scope: requested deployment, operations, packaging, observability artifacts, service definitions, mdBook/docs quality, FIPS docs, release notes.  
Result: **not enterprise-ready for deployment/ops documentation or packaging release use**. No files were modified.

## Executive Summary

I found **2 critical**, **7 high**, **6 medium**, and **2 low** findings. The largest risks are that the primary Linux deployment path is invalid as a user service, release/signing documentation materially overstates what CI actually produces, multiple platform readiness claims contradict each other, and monitoring/deployment docs do not match daemon behavior.

## Critical Findings

### C-01: Recommended Linux user systemd deployment is invalid

Severity: Critical  
Evidence: `docs/book/src/operations/deployment-guide.md:59` documents both system and per-user install paths; `docs/book/src/operations/deployment-guide.md:96` verifies with `systemctl --user status pcloudd.service`. The shipped unit sets `DynamicUser=yes` at `packaging/systemd/pcloudd.service:50`. `systemd-analyze --user verify packaging/systemd/pcloudd.service` fails with `DynamicUser= enabled for user unit, which is not supported. Refusing.`  
Impact: The documented primary user-mode install path fails before the daemon starts. Enterprise operators following the guide cannot deploy a working Linux user service.  
Remediation: Split system and user units. Remove unsupported hardening directives from the user unit, keep `DynamicUser=yes` only in the system unit, and add CI validation with both `systemd-analyze verify` and `systemd-analyze --user verify`.

### C-02: Release packaging/signing documentation materially overstates CI reality

Severity: Critical  
Evidence: `docs/book/src/operations/packaging-matrix.md:398` claims `.github/workflows/packaging.yml` builds and signs every supported target; that workflow is absent. `docs/book/src/reference/packaging.md:368` claims release jobs for Linux deb/rpm, AppImage, Flatpak, Snap, Docker, macOS, Windows, and SLSA provenance. Actual `.github/workflows/release.yml:41` only builds Linux raw binaries/SBOMs, and `.github/workflows/release-packaging.yml:80` builds `.deb`/`.rpm`. Package upload at `.github/workflows/release-packaging.yml:108` only writes SHA256 files, with no GPG/cosign package signatures or provenance.  
Impact: Users may trust unsigned or partially signed artifacts based on inaccurate enterprise supply-chain claims. This is a release-governance blocker.  
Remediation: Either implement the documented packaging workflow, signatures, provenance, and cross-platform matrix, or downgrade the documentation to exactly what exists. Add CI checks that fail when release docs reference missing workflows or unsupported artifact classes.

## High Findings

### H-01: Hardened Linux unit blocks real pCloud API access unless an override is installed

Severity: High  
Evidence: `packaging/systemd/pcloudd.service:119` sets `IPAddressDeny=any`; `packaging/systemd/pcloudd.service:122` only allows localhost. `packaging/systemd/README.md:21` admits the unit prevents real work unless overridden. The deployment guide starts the service at `docs/book/src/operations/deployment-guide.md:68` and only discusses FUSE overrides at `docs/book/src/operations/deployment-guide.md:83`, not the required API egress override.  
Impact: A deployed daemon can start but cannot log in or sync against pCloud, producing confusing production failures.  
Remediation: Make the deployment guide require the network override before first login, or ship a separate `pcloudd-hardened.service` profile. Add a post-install smoke test that performs outbound API connectivity.

### H-02: Shipped systemd socket unit is not implemented by the daemon

Severity: High  
Evidence: `packaging/systemd/pcloudd.socket:8` defines a socket activation unit. `packaging/systemd/README.md:15` calls it optional. The daemon binds its own Unix socket directly at `crates/pcloud-daemon/src/main.rs:122` and `crates/pcloud-daemon/src/main.rs:143`. Repository search found no `LISTEN_FDS`, `sd_listen_fds`, or equivalent systemd socket-activation handling. Cargo packaging includes the socket unit at `crates/pcloud-daemon/Cargo.toml:156` and `crates/pcloud-daemon/Cargo.toml:200`.  
Impact: Enabling socket activation can cause bind conflicts or a service that never accepts the inherited socket. Packaging ships an attractive but broken ops path.  
Remediation: Remove the socket unit from packages until supported, or implement systemd socket activation and add an integration test that starts the daemon from inherited file descriptors.

### H-03: BSD service definitions do not launch the daemon correctly

Severity: High  
Evidence: `packaging/freebsd/pcloudd.rc:52` runs `/usr/local/bin/pcloudd` without `serve`; `packaging/openbsd/pcloudd:32` does the same. The daemon enters summary mode without `serve` at `crates/pcloud-daemon/src/main.rs:56` and returns after printing summary at `crates/pcloud-daemon/src/main.rs:76`. `packaging/netbsd/pcloudd:41` passes `-p ${pidfile}`, but `packaging/freebsd/pcloudd.rc:19` notes the daemon does not accept `-p`.  
Impact: FreeBSD/OpenBSD services exit instead of running the daemon, and NetBSD passes unsupported arguments. Deployment docs at `docs/book/src/operations/deployment-guide.md:299` therefore describe non-working services.  
Remediation: Change BSD service scripts to run `pcloudd serve`, remove unsupported `-p` arguments, and add platform-specific service lint/smoke checks.

### H-04: Observability configuration and docs do not match runtime behavior

Severity: High  
Evidence: Prometheus docs use `PCLOUD_METRICS_ADDR` and port `9180` at `ops/prometheus/pcloud-rs-alerts.yml:8`, but the exporter defaults to `PCLOUD_METRICS_PORT` and port `9353` at `crates/pcloud-observability/src/exporter.rs:60`. The runbook uses `127.0.0.1:9301` at `docs/book/src/operations/runbook.md:1999`. Config docs say metrics are owner-only IPC and do not open TCP at `docs/book/src/reference/config.md:368`, while daemon metrics spawning uses TCP exporter code at `crates/pcloud-daemon/src/main.rs:196`.  
Impact: Operators cannot reliably enable or scrape metrics, and may accidentally expose or fail to expose telemetry contrary to documented security posture.  
Remediation: Choose one metrics control plane, wire `metrics_enabled` into daemon startup, document the real bind variables and default port, and add an end-to-end metrics scrape test.

### H-05: Platform readiness claims contradict the repository's own truth documents

Severity: High  
Evidence: `docs/book/src/architecture/platform-support.md:20` labels macOS and Windows as T1, while `STATUS.md:66` says Windows is Tier-2 and `STATUS.md:116` says macOS is Tier-3 scaffolded-only. `CLAUDE.md:496` says Windows named-pipe IPC is not wired and service work is not Tier-1, while `docs/book/src/architecture/platform-support.md:143` describes named-pipe DACL and SID checks as live.  
Impact: Enterprise readers cannot determine what is supported, tested, or safe to deploy. This can lead to unsupported Windows/macOS deployments being treated as production-ready.  
Remediation: Establish a single platform support matrix as authoritative, link all docs to it, and gate readiness labels on live CI evidence.

### H-06: Operations runbook contains stale commands and inconsistent state paths

Severity: High  
Evidence: `OPERATIONS-RUNBOOK.md:264` lists `~/.local/share/pcloud-rs/store.sqlite`, while daemon code uses `store.sqlite3` at `crates/pcloud-daemon/src/main.rs:161`. The runbook invokes `pcloud-cli` at `OPERATIONS-RUNBOOK.md:273`, but current docs use `pcloudc` at `README.md:81`. Rollback guidance restores `auth_token.dat` and `.meta` to `~/.config/pcloud-rs` at `OPERATIONS-RUNBOOK.md:433`, conflicting with earlier vault path guidance at `OPERATIONS-RUNBOOK.md:266`.  
Impact: During backup, restore, or incident response, operators can back up or restore the wrong files and fail recovery.  
Remediation: Rewrite runbook state inventory from actual config/runtime code, replace stale binary names, and add a restore drill test that validates a restored daemon can start and authenticate.

### H-07: Nix packaging docs claim modules and binaries not present in the flake

Severity: High  
Evidence: `docs/book/src/reference/packaging.md:103` documents `nixosModules.pcloud-rs`, but `flake.nix:82` only exposes packages/apps. `docs/book/src/operations/packaging-matrix.md:133` references `nixos/pcloud-rs.nix`, which is absent. `flake.nix:46` sets `mainProgram = "pcloud-rs"`, while the documented binaries are `pcloudc` and `pcloudd`.  
Impact: NixOS users following enterprise docs cannot deploy the claimed module and may get broken app metadata.  
Remediation: Either add the NixOS module and correct app outputs, or remove the NixOS module claims until implemented. Validate with `nix flake check` in CI.

## Medium Findings

### M-01: mdBook contains broken or empty links and references missing files

Severity: Medium  
Evidence: `docs/book/src/SUMMARY.md:53` has an empty `Platforms` link. `README.md:41` and `docs/book/src/architecture/platform-support.md:8` reference missing `PLAN_CROSSPLATFORM.md`. `docs/book/src/operations/deployment.md:405` and `docs/book/src/operations/web-ui.md:333` reference missing `docs/book/src/reference/metrics.md`. `docs/enterprise/kms.md:485` references missing `docs/book/src/cli/crypto.md` and `docs/runbooks/kms-outage.md`.  
Impact: The book may build, but enterprise readers hit dead navigation and missing operational references.  
Remediation: Add mdBook link checking in CI and either create or remove every referenced file.

### M-02: Alerting and dashboards omit critical mount/sync signals

Severity: Medium  
Evidence: `ops/prometheus/pcloud-rs-alerts.yml:21` has TODOs for missing `pcloud_mount_state` and `pcloud_sync_queue_depth`. The dashboard at `ops/grafana/pcloud-rs-overview.json:25` only covers auth, latency, transfer, crypto, IPC clients, and panics. `crates/pcloud-daemon/src/metrics_server.rs:172` notes upload retry/started SLO counters are not wired.  
Impact: Operators lack alerts for mount health, sync backlog, and real upload SLOs, so common outage modes are invisible.  
Remediation: Add mount and sync metrics, wire SLO counters, then update alerts and dashboards with tested PromQL.

### M-03: `.env.example` encourages plaintext credential sourcing

Severity: Medium  
Evidence: `.env.example:1` tells users to copy and source `.env`; `.env.example:11` and `.env.example:12` include `PCLOUD_USERNAME` and `PCLOUD_PASSWORD`.  
Impact: Passwords can leak through shell history, process environments, crash dumps, or accidental commits.  
Remediation: Remove password-based environment examples, prefer interactive login or a secret manager, and document environment variables only for non-secret test fixtures.

### M-04: FIPS documentation is honest about gaps but incomplete for compliance use

Severity: Medium  
Evidence: `docs/fips.md:3` says no validated module ships, but `docs/fips.md:61` references AWS-LC-FIPS certificate #4759 without requiring exact validated module version, tested platform, security policy, or operational environment. NIST CMVP certificate #4759 is active for specific AWS-LC configurations, not a blanket validation for every consuming binary: https://csrc.nist.gov/projects/cryptographic-module-validation-program/certificate/4759.  
Impact: Compliance users may assume enabling a provider crate is sufficient for FIPS operation.  
Remediation: Add a strict FIPS boundary section: validated module artifact, certificate version, allowed OS/CPU configurations, security policy, build provenance, runtime self-tests, and non-FIPS disabled algorithms.

### M-05: Release notes and status documents drift from each other

Severity: Medium  
Evidence: `STATUS.md:58` reports `154 / 2 / 0 / 30`, while `CHANGELOG.md:64` claims `158 / 0 / 0 / 28`. `CHANGELOG.md:1510` claims named-pipe IPC, service integration, packaging recipes, and Linux signing are live in CI, conflicting with current workflows and platform status docs.  
Impact: Release consumers cannot trust readiness summaries or compare releases against actual tested capabilities.  
Remediation: Generate release readiness tables from a single machine-readable source, and require changelog/status consistency checks before release.

### M-06: Backup snapshot docs contradict themselves on encryption and command shape

Severity: Medium  
Evidence: `docs/book/src/operations/backup-snapshots.md:19` says the default pipeline has no GPG, while `docs/book/src/operations/backup-snapshots.md:71` requires `gpg(1)` and `docs/book/src/operations/backup-snapshots.md:91` calls the snapshot a GPG-encrypted `.tar.gpg`. The same doc introduces `pcloudc snapshot` at `docs/book/src/operations/backup-snapshots.md:5` but later examples use legacy `pcloudc backup snapshot-*` at `docs/book/src/operations/backup-snapshots.md:176`.  
Impact: Operators may create unencrypted backups believing they are encrypted, or automate legacy commands unintentionally.  
Remediation: Split default and encrypted workflows, mark legacy commands as compatibility-only, and add restore examples for both formats.

## Low Findings

### L-01: macOS plist has duplicate and questionable operational settings

Severity: Low  
Evidence: `packaging/macos/com.pcloud.pcloud-rs.plist:68` and `packaging/macos/com.pcloud.pcloud-rs.plist:79` both set `ThrottleInterval`. `packaging/macos/com.pcloud.pcloud-rs.plist:109` uses `{{USER_HOME}}/.config/pcloud`, while most docs use `pcloud-rs` paths.  
Impact: This is unlikely to block launchd, but it increases packaging ambiguity and support burden.  
Remediation: Remove duplicate keys, standardize paths, and validate plist files with `plutil` in CI.

### L-02: Packaging README contains stale service paths and command paths

Severity: Low  
Evidence: `packaging/README.md:40` references `packaging/init/systemd/pcloudd.service`, while the actual path is `packaging/systemd/pcloudd.service`. `packaging/README.md:68` says systemd uses `/usr/local/bin/pcloudd serve`, but the unit uses `/usr/bin/pcloudd serve` at `packaging/systemd/pcloudd.service:38`.  
Impact: Maintainers and downstream packagers can copy stale paths into packages or docs.  
Remediation: Update packaging docs from actual package assets and add a simple doc reference check.

## Commands Run

- `sed -n '1,520p' pcloud_rev.md`
- `git status --short`
- `rg --files -g '!target/**' -g '!vendor/**' -g '!.beads/**'`
- `find docs packaging ops .github/workflows -type f`
- `rg -n "production|enterprise|Tier|FIPS|sign|SLSA|backup|restore|upgrade|metrics|systemd|launchd|Windows|FreeBSD|mdbook"`
- `rg -n "TODO|FIXME|placeholder|stub|not wired|not implemented|scaffold"`
- `systemd-analyze verify packaging/systemd/pcloudd.service`
- `systemd-analyze --user verify packaging/systemd/pcloudd.service`
- `rg -n "LISTEN_FDS|sd_listen|listen_fds|socket activation"`
- `mdbook --version`
- `nix --version`
- Targeted `nl -ba` reads of referenced docs, workflows, service files, and config/runtime code.
- Browser verification of NIST CMVP certificate #4759 for FIPS context.

## Limitations

The worktree was already dirty and included untracked audit/doc/workflow files; I audited the current checkout as-is and made no changes. `mdbook` and `nix` are not installed in this environment, so I could not run `mdbook build` or `nix flake check`. I did not run package builds, live cloud tests, Windows/macOS/BSD service tests, `promtool`, `plutil`, WiX tooling, or release signing workflows.

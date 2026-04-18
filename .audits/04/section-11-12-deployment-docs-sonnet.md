# Sections 11 & 12 Audit — Deployment & Documentation
**Auditor:** Claude Sonnet 4.6 (independent cross-validator)
**Date:** 2026-04-18
**Scope:** Packaging, CI workflows, docs, runbooks, deployment guide, SDK reference, parity/status docs

---

## Summary

The deployment and documentation surface is in materially better shape than the prior AUDIT_REPORT.md baseline (which found zero CI workflows). CI now exists for all four tier-1 platforms. The systemd unit is hardened. Packaging recipes are real. Docs maintain honest parity disclaimers throughout. Seven findings remain, two of them HIGH.

---

## CRITICAL [0]

None identified.

---

## HIGH [2]

### H-1 — FreeBSD CI is non-blocking; `ci.yml` comment claims it "validates the FreeBSD tier-1 claim"

**File:** `.github/workflows/ci.yml:68-73`

The FreeBSD job carries `continue-on-error: true`, meaning a full FreeBSD build/test failure still produces a green CI gate. The inline comment reads _"FreeBSD CI — exercises pcloud-fs/src/platform/bsd.rs and **validates the FreeBSD tier-1 claim**"_ while the very next line makes the job non-blocking. A non-blocking job cannot validate a tier-1 claim. If FreeBSD CI fails silently, PRs merge with broken BSD support undetected. The packaging matrix docs correctly classify FreeBSD as T2/scaffolded (`docs/book/src/operations/packaging-matrix.md`), but the CI comment contradicts that.

**Remediation:** Either (a) remove `continue-on-error: true` and accept FreeBSD as a hard gate once the FUSE parity proof is closed, or (b) reword the comment to _"intended future tier-1 evidence; non-blocking until bd-1du.4 hardware verification completes"_. Do not claim a non-blocking job is a tier-1 validator.

---

### H-2 — `Type=notify` systemd unit lacks `NotifyAccess=`; WiX installer has unresolved signing TODOs

**Files:** `packaging/systemd/pcloudd.service` (entire file); `packaging/windows/wix/pcloud-rs.wxs:5-6`

**Part A:** The systemd unit specifies `Type=notify` and documents that the daemon emits `READY=1` / `WATCHDOG=1` via a custom `sd_notify()` function in `crates/pcloud-daemon/src/serve.rs:41`. However, the unit file does not set `NotifyAccess=`. For user units with `DynamicUser=yes` and `PrivateUsers=yes`, the default `NotifyAccess=none` will silently drop all `sd_notify` datagrams. The watchdog mechanism will not work, and systemd will wait indefinitely for the `READY=1` signal before considering the service started, causing a timeout failure on `systemctl start pcloudd`. Setting `NotifyAccess=main` is required.

**Part B:** `pcloud-rs.wxs` contains two unresolved `TODO` comments: (1) `SigningCertificatePath` is not set — the MSI ships unsigned; (2) `UpgradeCode` GUID must be stabilised before first release. The packaging matrix docs acknowledge the signing stub, but the WiX file itself has no CI guard preventing an unsigned MSI from being shipped inadvertently.

**Remediation A:** Add `NotifyAccess=main` to `[Service]` in `pcloudd.service`.
**Remediation B:** Before any tagged release, resolve the WiX signing TODO via a build-time CI secret; add a CI step that fails if the MSI is unsigned.

---

## MEDIUM [3]

### M-1 — No Prometheus alert rules or Grafana dashboards shipped

**Files:** `packaging/` (all subdirectories); `docs/book/src/` (all files)

The deployment guide and observability section of the runbook describe Prometheus metrics exported by `pcloud-observability`, but no `rules.yml`, `alerts.yml`, or Grafana dashboard JSON files exist anywhere in the tree. Operators following the deployment guide have no starting-point alert coverage. The pcloud_rev.md audit scope explicitly asks _"Dashboards shipped? Alert rules?"_ — the answer is no.

**Remediation:** Ship at minimum a starter `packaging/prometheus/pcloud-rs.rules.yml` with alerts for daemon down, high IPC error rate, and stalled sync queue. A companion Grafana JSON is strongly recommended. Track under `bd-1du` or a new bead.

---

### M-2 — `IPAddressAllow=localhost` in systemd unit blocks pCloud API access by default

**File:** `packaging/systemd/pcloudd.service:77-79`

The unit sets `IPAddressAllow=localhost` with an inline comment instructing operators to add a drop-in override for pCloud API endpoints. A daemon that cannot reach `binapi.pcloud.com` / `eapi.pcloud.com` on first install will produce confusing connection errors. The comment acknowledges this, but no drop-in override template or `pcloudd.service.d/` example is shipped alongside the unit, leaving operators without a concrete next step.

**Remediation:** Add `packaging/systemd/pcloudd.service.d/api-access.conf.example` that broadens `IPAddressAllow=` to the known pCloud API CIDRs, with a comment noting it must be activated manually. Reference it from the deployment guide.

---

### M-3 — STATUS.md headline and body are inconsistent: 158/0/0/28 vs 156/2/0/28

**File:** `STATUS.md:7-36` vs `STATUS.md:43-83`

`STATUS.md` opens with a 2026-04-18 section declaring _"Headline now: 158 / 0 / 0 / 28"_ (rows 93 and 149 flipped to Implemented). The immediately following section (also dated 2026-04-18) states _"The CSV is authoritative and now reads 156 / 2 / 0 / 28"_ and explains the first headline was wrong. The file never resolves which number is canonical for the reader landing on line 1. A sysadmin or auditor reading only the first screen of `STATUS.md` takes away the wrong (158) count. `CLAUDE.md` correctly says "single source of truth: STATUS.md" but a reader has to scroll past a self-correction to get the accurate number.

**Remediation:** Reorder `STATUS.md` so the most recent, accurate count (158/0/0/28 after the IPC wiring landed, or 156/2/0/28 if that work is not yet merged) appears first as an unambiguous header, with prior superseded entries clearly archived below.

---

## LOW [2]

### L-1 — `cargo-deb.toml` is a reference snippet, not a wired packaging artifact

**File:** `packaging/debian/cargo-deb.toml`

The file is self-described as _"NOT auto-consumed by cargo-deb"_ and instructs maintainers to manually copy a snippet into the crate's `Cargo.toml`. This means Debian packaging is not reproducibly exercised by CI and cannot be cut without manual intervention. The preferred path (`nfpm.yaml`) exists alongside it, but the dual-path situation may confuse contributors.

**Remediation:** Either remove `cargo-deb.toml` and document `nfpm.yaml` as the sole Debian path, or wire `cargo-deb` in a CI job and delete the manual-copy instruction.

---

### L-2 — macOS launchd plist silently sets env vars the daemon does not read

**File:** `packaging/macos/com.pcloud.pcloudd.plist:28-31`

The plist comment correctly notes that `PCLOUD_HOME`, `PCLOUD_CONFIG`, `PCLOUD_AUTH_VAULT`, `PCLOUD_IPC_SOCKET`, `PCLOUD_API_SERVER` are _"NOT read by the Rust daemon"_ and _"silently ignored"_. Setting non-functional environment variables in a production plist is noise that misleads operators debugging startup failures. The comment is honest, but the correct fix is removal.

**Remediation:** Remove the five "compat alias" keys from the plist's `EnvironmentVariables` dict. If they are needed for a future migration tool, document them in a separate migration note rather than in the live service plist.

---

## Documentation Integrity Assessment

- `README.md` correctly disclaim pre-alpha status and the `bd-1du.10` gate condition. No false "production ready" or "full parity" claims detected.
- `OPERATIONS-RUNBOOK.md` opens with an explicit non-production-readiness disclaimer referencing open beads.
- `docs/book/src/operations/deployment.md` carries an _"Honesty callout"_ referencing `STATUS.md`.
- `docs/book/src/getting-started/install.md` labels the project **pre-alpha** and correctly describes mount scaffolding vs live status.
- `packaging/operations/packaging-matrix.md` distinguishes T1 (Linux), T2 (FreeBSD), with appropriate scaffolded disclaimers for macOS/Windows mounts.
- `#![deny(missing_docs)]` is enforced across all major crates — rustdoc coverage is gated at compile time.
- SQLite schema migrations are implemented with forward-only versioned migrations in `pcloud-store/src/schema.rs` (versions 1–5 confirmed).

No false "production ready", "full parity", "enterprise ready", or "drop-in replacement" claims were found in any documentation file audited.

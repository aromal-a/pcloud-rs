# Audit 06 — Sections 11 & 12: Deployment & Docs
**Auditor**: Sonnet (independent cross-validator of Opus audit-05)
**Date**: 2026-04-18
**Status of post-audit-05 doc fixes**: verified held

---

## Section 11 — Deployment & Operations

### MEDIUM — M-11-1: launchd plist missing `ExitTimeOut` key

**File**: `packaging/macos/com.pcloud.pcloudd.plist`

The audit requirement (`ExitTimeOut`) is a launchd key that sets the number
of seconds launchd waits before force-killing a service that did not exit
after receiving SIGTERM. The shipped plist has `KeepAlive` (crash-restart
policy) but no `ExitTimeOut`. Without it, launchd uses a default of 20 s,
which may truncate the daemon's graceful-drain window on hosts with large
sync queues. Not a security gap, but an operational gap for production
deployments.

**Remediation**: add `<key>ExitTimeOut</key><integer>30</integer>` to
`com.pcloud.pcloudd.plist`, matching the 30 s `TimeoutStopSec` in the
systemd unit.

---

### MEDIUM — M-11-2: launchd plist missing `ThrottleInterval`

**File**: `packaging/macos/com.pcloud.pcloudd.plist`

The plist has no `ThrottleInterval` key. If the daemon crashes during early
startup (e.g., IPC socket bind failure), launchd will restart it immediately
in a tight loop, burning CPU. A minimum of 10 s is standard.

**Remediation**: add `<key>ThrottleInterval</key><integer>10</integer>`.

---

### MEDIUM — M-11-3: systemd unit `IPAddressAllow=localhost` blocks real pCloud API

**File**: `packaging/systemd/pcloudd.service`, line 91–93

```
IPAddressAllow=localhost
# Operators MUST broaden IPAddressAllow= to cover the pCloud API endpoints
# via a drop-in override.
```

The comment is correct and the intent is sound, but the base unit ships in
a state where the daemon cannot reach `api.pcloud.com` or
`eapi.pcloud.com` without a drop-in override. There is no shipped
`override-api.conf.example` file analogous to `override-fuse.conf.example`.
An operator who installs the package and does not read the inline comment
will observe silent failures connecting to the API.

**Remediation**: ship an `override-api.conf.example` that sets
`IPAddressAllow=` to the two canonical API domains, with the same
documentation discipline used for the FUSE override.

---

### LOW — L-11-4: logrotate postrotate HUP signal vs. daemon's signal handling

**File**: `packaging/debian/pcloud-rs.logrotate`

The `postrotate` script sends `HUP` (`systemctl kill -s HUP pcloudd.service`)
to trigger log handle re-open. The daemon's OPERATIONS-RUNBOOK.md describes
signal handlers for `SIGTERM`/`SIGINT` only. There is no documentation that
`SIGHUP` causes a log re-open (as opposed to a daemon restart). If the
daemon treats `SIGHUP` as unhandled, the kernel default action is to
terminate it.

**Remediation**: either document that `SIGHUP` is handled for log re-open in
`OPERATIONS-RUNBOOK.md` and wire it in the daemon signal handler, or change
the logrotate stanza to use `copytruncate` which avoids sending any signal.

---

### LOW — L-11-5: FIPS posture not documented; no plan for FIPS-validated provider

**Files**: `SECURITY-MODEL.md`, `docs/book/src/architecture/security-model.md:283`

The audit scope item "if claimed, verify the crypto backend can switch to a
FIPS-validated provider" is not claimed anywhere in the project docs, and the
single FIPS mention in the book notes the project has "no FIPS constraint."
This is honest. However, the enterprise deployment guide (`docs/enterprise/`)
has no entry documenting the FIPS non-posture, which is relevant for
regulated sectors evaluating the tool.

**Remediation**: add a one-paragraph FIPS disclaimer to
`docs/enterprise/README.md` confirming FIPS 140 is not targeted and linking
to the security model.

---

### LOW — L-11-6: WiX installer references `pcloud-rs.ico` not shipped in tree

**File**: `packaging/windows/wix/pcloud-rs.wxs`, line 41

`<Icon Id="pcloud-rs.ico" SourceFile="pcloud-rs.ico" />` references an icon
file that does not exist in `packaging/windows/wix/`. The WiX build will
fail when run from a clean checkout without additional assets.

**Remediation**: ship a placeholder icon or add a CI guard that provides the
asset before invoking `candle.exe`/`light.exe`, and document this in
`packaging/windows/wix/README.md`.

---

## Section 12 — Documentation Quality

### LOW — L-12-1: CLAUDE.md parity counts lag STATUS.md by two audit rounds

**File**: `CLAUDE.md` (Current Truth section, 2026-04-18, post Audit 03)

`CLAUDE.md` states: **156 Implemented / 2 Partial / 0 Missing / 28 Rejected
(186 rows)** (Audit 03 figures). The current authoritative count in
`STATUS.md` is **153 / 5 / 0 / 28** (post audit-05). The handoff document
is two audit rounds stale on the headline count. This is a documentation
correctness gap, not a code gap; but a follow-on agent reading only `CLAUDE.md`
will mis-state the parity position.

**Remediation**: update the "Current Truth" section of `CLAUDE.md` to reflect
the audit-05 count (153/5/0/28) and note the five Partial rows (93, 26, 27,
124, 142). This is a one-paragraph edit.

---

### LOW — L-12-2: `docs/book/src/parity/status.md` is a mirror of STATUS.md with no sync mechanism

**File**: `docs/book/src/parity/status.md`

Both files carry the "do not claim full parity" discipline correctly. However,
there is no `include!` or generation step that keeps them synchronized. A
future audit wave that updates `STATUS.md` may leave the book chapter stale.
This is a process gap rather than an immediate accuracy error.

**Remediation**: add a CI step that asserts the headline count line in
`docs/book/src/parity/status.md` matches `STATUS.md`, or replace the book
chapter body with a verbatim mdBook `{{#include ../../../../STATUS.md}}`.

---

### LOW — L-12-3: Deployment guide lacks "backup what state" checklist section

**File**: `docs/book/src/operations/deployment.md`

The audit requirement "Backup / restore: documented state that needs to be
backed up (vault, SQLite, journal, mount orphan registry)" is partially
covered by `docs/book/src/operations/backup-snapshots.md`. The deployment
chapter itself has no cross-reference to that chapter in its Day-2
operations section. A sysadmin following only the deployment chapter will
miss the vault backup step.

**Remediation**: add a "State backup" cross-reference paragraph to
`docs/book/src/operations/deployment.md` pointing at
`backup-snapshots.md` and listing the four state components (vault, SQLite
store, append-only journal, mount orphan registry).

---

## Summary table

| ID | Severity | Area | File |
|----|----------|------|------|
| M-11-1 | MEDIUM | Deployment | `packaging/macos/com.pcloud.pcloudd.plist` |
| M-11-2 | MEDIUM | Deployment | `packaging/macos/com.pcloud.pcloudd.plist` |
| M-11-3 | MEDIUM | Deployment | `packaging/systemd/pcloudd.service:91-93` |
| L-11-4 | LOW | Deployment | `packaging/debian/pcloud-rs.logrotate` |
| L-11-5 | LOW | Deployment | `docs/enterprise/README.md` (missing entry) |
| L-11-6 | LOW | Deployment | `packaging/windows/wix/pcloud-rs.wxs:41` |
| L-12-1 | LOW | Docs | `CLAUDE.md` (Current Truth section) |
| L-12-2 | LOW | Docs | `docs/book/src/parity/status.md` |
| L-12-3 | LOW | Docs | `docs/book/src/operations/deployment.md` |

**Findings held from audit-05**: The "production ready / full parity" ban is
correctly enforced across README, CHANGELOG, STATUS.md, CLAUDE.md, book
introduction, FAQ, deployment chapter, and architecture overview — no
violations found. Systemd unit hardening (ProtectSystem, PrivateDevices,
MemoryMax, CPUQuota, WatchdogSec, FUSE override drop-in) is well-formed.
FreeBSD rc.d script is present and functionally correct. Windows SCM
ServiceInstall/ServiceControl is wired in the WiX file. Prometheus alert
rules and Grafana dashboard JSON are present. OTel tracing is documented in
`docs/enterprise/tracing.md`. Health and readiness endpoints (`/health`,
`/readyz`) are implemented in `crates/pcloud-web/src/routes.rs`. No
CRITICAL or HIGH findings in §11–12 post audit-05 corrections.

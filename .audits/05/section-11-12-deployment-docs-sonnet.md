# Audit 05 — Sections 11 & 12: Deployment & Documentation (Sonnet)

Date: 2026-04-18
Auditor: Claude Sonnet 4.6 (independent cross-validation with Opus)
Scope: `packaging/`, `docs/book/`, root `.md` files, `.github/workflows/`,
`STATUS.md`, `CLAUDE.md`, `API-REFERENCE.md`, `OPERATIONS-RUNBOOK.md`,
`docs/CRYPTO-BACKEND-PLAN.md`, `docs/enterprise/crypto-compat.md`,
`docs/crypto-reference-pclsync.md`

## CSV Authoritative Count

Python parse of `C_FEATURE_PARITY_MATRIX.csv` (186 data rows):
**153 Implemented / 5 Partial / 0 Missing / 28 Rejected**
Matches STATUS.md:23 headline. Used as ground truth below.

---

## HIGH

### H-1 — CLAUDE.md hard-codes stale parity headline (156/2/0/28)

`CLAUDE.md:66-67` states "Audit 03 (2026-04-18) reconciled the matrix:
**156 Implemented / 2 Partial / 0 Missing / 28 Rejected (186 rows)**"
and names only rows 93 and 149 as Partial. The actual CSV count after
audit-04 is 153/5/0/28 (rows 26, 27, 93, 124, 142 are Partial).
`CLAUDE.md:366` repeats the same stale "156 Implemented / 2 Partial"
claim. CLAUDE.md's own documentation-discipline rule at the "Current
Truth" section says to link to STATUS.md and not hard-code counts — the
very paragraph under "Current Truth" violates its own rule.

Confirms: Opus H-1.

**Remediation:** remove or replace the hard-coded "156 / 2 / 0 / 28"
paragraph at `CLAUDE.md:66-70` and `CLAUDE.md:364-370` with a neutral
"see STATUS.md". Update the open-beads list to note 5 Partial rows.

### H-2 — Systemd unit blocks FUSE with no drop-in guidance

`packaging/systemd/pcloudd.service:49` sets `PrivateDevices=yes`, which
removes `/dev/fuse` from the mount namespace. Line 89 explicitly filters
out the `@mount` syscall group. FUSE requires both `/dev/fuse` access
and `mount(2)`. The comment at line 71 says "FUSE mount (if enabled)
should be declared per-deployment" but there is no example drop-in for
FUSE, unlike the `override.conf.example` provided for `IPAddressAllow`.
A deployer following the packaging README will get a silently broken
FUSE mount with no diagnostic. The Linux FUSE path is the primary
feature differentiator and one of the two tier-1 parity claims.

**Remediation:** ship a `fuse-override.conf.example` drop-in alongside
`override.conf.example` containing:

```ini
[Service]
PrivateDevices=no
SystemCallFilter=@mount
ReadWritePaths=/dev/fuse /run/user/%U
```

with a comment referencing the FUSE bead `bd-1du.4`.

### H-3 — STATUS.md has three mutually-contradictory headline counts

- `STATUS.md:23` — **153 / 5 / 0 / 28** (matches CSV, current)
- `STATUS.md:66` — "**155 / 3 / 0 / 28**"
- `STATUS.md:82-87` — "**156 / 2 / 0 / 28**"

Lines 66 and 82 have no clear archive/superseded marking that a new
reader will parse. All three claim to be the 2026-04-18 state. A
deployer skimming for the parity count could land on any of the three.

Confirms: Opus H-2.

**Remediation:** move lines 66-123 (the 155/3 section) and lines 76-128
(the 156/2 section) under a clearly-fenced "Superseded audit history"
block; leave the 153/5 paragraph as the single unambiguous header.

---

## MEDIUM

### M-1 — `docs/CRYPTO-BACKEND-PLAN.md` header says "Planning. No code changes yet"

`docs/CRYPTO-BACKEND-PLAN.md:3` reads:
`Status: Planning. No code changes yet. Wave 1 primitives (A–F) are being built in parallel; Wave 2 executes this plan once they land.`

Wave 2 is fully implemented (`crates/pcloud-crypto/src/lib.rs:159` has
the `CryptoBackend` enum; `pclsync_compat_profile.rs` is complete;
STATUS.md:7-22 declares Wave 2 landed and a live KAT passed). The plan
doc's header is now actively misleading — a new contributor reading it
will believe the backend dispatch is unimplemented.

**Remediation:** update the `Status:` line to reflect the shipped state,
e.g. "Status: Implemented (Wave 2 shipped 2026-04-18). This document
records the design intent; see `crates/pcloud-crypto/` for code."

### M-2 — launchd plist `com.pcloud.pcloudd.plist` missing `ExitTimeOut`

`packaging/macos/com.pcloud.pcloudd.plist` has no `ExitTimeOut` key.
Without it, launchd defaults to 20 seconds and SIGKILLs the daemon if
the graceful shutdown takes longer. `packaging/macos/com.pcloud.pcloud-rs.plist`
does include `ExitTimeOut` (line 65). There are two plists and they
diverge on this load-bearing key. The system-daemon plist is the one
that lacks it.

**Remediation:** add `<key>ExitTimeOut</key><integer>30</integer>` to
`packaging/macos/com.pcloud.pcloudd.plist` and reconcile/deduplicate
the two plists — one of them is redundant.

### M-3 — `API-REFERENCE.md` Partial catalogue only lists row 93

`API-REFERENCE.md:57-69` documents row 93 (`upload_writefromfile`) as
Partial but omits the four other Partials after audit-04: rows 26
(`psync_tfa_has_devices`), 27 (`psync_tfa_type`), 124
(`psync_crypto_share_folder`), 142 (`psync_crypto_account_teamshare`).
A deployer using the API reference as a capability map will miss the
TFA-type introspection gap and the crypto share-invite interop gap,
which are the most operationally visible.

Confirms: Opus M-1.

**Remediation:** add Partial rows for rows 26/27 under the Auth table
and rows 124/142 under Shares, citing the bead and the symmetric-
HMAC-vs-RSA-4096 rationale from STATUS.md:23-28.

### M-4 — macOS launchd plist exports five env vars the daemon ignores

`packaging/macos/com.pcloud.pcloudd.plist:97-106` exports
`PCLOUD_HOME`, `PCLOUD_CONFIG`, `PCLOUD_AUTH_VAULT`, `PCLOUD_API_SERVER`,
`PCLOUD_IPC_SOCKET`. The inline comment at lines 22-31 correctly states
these are "NOT read by the Rust daemon". Shipping unread config in a
deployment artifact is a reliability trap: a sysadmin changing
`PCLOUD_AUTH_VAULT` here will expect the vault to move and will open a
bug when it does not.

Confirms: Opus M-3.

**Remediation:** remove the five compat-alias keys from the plist;
retain only `PCLOUD_ROOT`, `PCLOUD_ENV`, `PCLOUD_LOG_LEVEL`,
`PCLOUD_API_HOST`, `PCLOUD_API_SERVER_NAME`.

### M-5 — No Prometheus dashboard, alert rules, or Grafana files shipped

`crates/pcloud-daemon/src/metrics_server.rs` exports a Prometheus
text endpoint and `render_prometheus()` is wired. The OPERATIONS-RUNBOOK
does not mention the `/metrics` endpoint URL, port, or how to configure
scrape targets. No `dashboards/` directory exists. `pcloud_rev.md:283`
lists "Prometheus metrics via pcloud-observability" as a strategic
deployment goal. Operators have no turnkey observability artifact.

**Remediation:** add a `dashboards/` directory with at minimum a
reference Grafana dashboard JSON and a Prometheus scrape config snippet;
document the default metrics port in OPERATIONS-RUNBOOK.md.

---

## LOW

### L-1 — `docs/crypto-reference-pclsync.md:304` documents a "Missing surface" for server API

`docs/crypto-reference-pclsync.md:304` states the Rust daemon "does not
implement the wire methods" for `crypto_setuserkeys /
crypto_getuserkeys`. This may be stale (the crypto backend landed
Wave 2). Should be cross-checked against the parity matrix and updated
or confirmed as a genuine gap with a bead reference.

**Remediation:** verify against actual proto methods and either update
the table to "Implemented" with a file:line citation, or open a bead
for the missing surface.

### L-2 — CHANGELOG has no version tags; all entries under `[Unreleased]`

`CHANGELOG.md:8-9` notes semver will apply "once the first tagged
release ships". The current accumulation under `[Unreleased]` is
acceptable pre-v0.1.0 but the release CI (`release.yml`) does not
validate that a CHANGELOG entry exists for the release version.

**Remediation:** add a CI check that fails if `[Unreleased]` is the
only heading when a release tag is pushed.

### L-3 — `cargo doc` gate absent from CI

Neither `ci.yml` nor any workflow runs `cargo doc --workspace --no-deps`.
Rustdoc warnings are therefore invisible in CI. Given that public-item
doc coverage is cited as an audit criterion (pcloud_rev.md:219-221),
this is a gap.

**Remediation:** add `cargo doc --workspace --no-deps 2>&1 | grep "^warning" && exit 1`
as a CI step.

---

## What Is Working Well

- **systemd unit** (`packaging/systemd/pcloudd.service`): one of the
  most hardened units reviewed. `DynamicUser=yes`, `CapabilityBoundingSet=`,
  deny-by-default `@privileged @mount @resources` syscall filter,
  `ProtectHome=tmpfs`, `IPAddressDeny=any`, `MemoryMax=512M`,
  `WatchdogSec=30s`, `Type=notify` with `NotifyAccess=main`. Strongly
  correct for a non-FUSE deployment.
- **Honesty discipline** is consistent across 9 files: no "production
  ready", "full parity", or "drop-in replacement" claims found anywhere.
- **mdbook CI gate** (`ci.yml:149-162`) installed and wired, so the
  book can't silently diverge.
- **WiX installer** (`packaging/windows/wix/pcloud-rs.wxs`) includes
  SCM `ServiceInstall`/`ServiceControl`, frozen `UpgradeCode` GUID,
  `PackageDependency Id="winfsp"` for dependency detection.
- **FreeBSD rc.d** (`packaging/freebsd/pcloudd.rc`) exists.
- **SELinux** (`packaging/selinux/pcloud-rs.te`) and **AppArmor**
  (`packaging/apparmor/usr.local.bin.pcloudd`) profiles both present.
- **Reproducible-build** dual-runner digest compare in `ci.yml:102-147`
  with `SOURCE_DATE_EPOCH` is real and fails on mismatch.
- **OPERATIONS-RUNBOOK.md** upgrade path and rollback procedures are
  documented concretely; vault backup is called out explicitly.

---

## Cross-Validation Summary

Findings corroborating Opus:

| Sonnet | Opus | Description |
|--------|------|-------------|
| H-1 | H-1 | CLAUDE.md stale 156/2/0/28 headline |
| H-3 | H-2 | STATUS.md three contradictory counts |
| M-3 | M-1 | API-REFERENCE.md missing 4 Partial rows |
| M-4 | M-3 | launchd plist exports ignored env vars |

Independent Sonnet findings:

| Finding | Description |
|---------|-------------|
| H-2 | systemd unit blocks FUSE with no drop-in override example |
| M-1 | CRYPTO-BACKEND-PLAN.md "Planning. No code changes yet" is stale |
| M-2 | launchd com.pcloud.pcloudd.plist missing ExitTimeOut |
| M-5 | No Prometheus dashboard/alert rules shipped |
| L-1 | crypto-reference-pclsync.md may have a stale "Missing surface" |
| L-2 | CHANGELOG has no release-gate CI check |
| L-3 | cargo doc gate absent from CI |

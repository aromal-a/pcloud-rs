# Audit 04 — Sections 11 & 12: Deployment & Documentation (Opus)

Date: 2026-04-18 | Auditor: Claude Opus 4.7 | Scope: `packaging/`, `.github/workflows/`, root docs, `docs/book/`.

## Executive summary

Packaging scaffolding is impressively broad (systemd, launchd, WiX, rc.d, nfpm, AppArmor, SELinux, signing scripts) and the systemd unit is genuinely hardened. CI covers three OSes plus FreeBSD and has a scheduled fuzz job, but reproducible-build verification and an SBOM pipeline are absent from CI. Documentation discipline on the honesty rule ("no full parity / production ready" claims) is enforced consistently across root .md files — no false parity claims were found. However, **API-REFERENCE.md is severely stale** relative to CLAUDE.md / STATUS.md reality and contradicts the claimed parity counts. That is the single biggest doc bug in this scope.

## CRITICAL

None that block security/data integrity within sections 11-12.

## HIGH

- **HIGH-1. `API-REFERENCE.md` is stale and contradicts STATUS.md / CLAUDE.md.**
  - `API-REFERENCE.md:45-53` marks `sync add` as `P (bd-1du.3)`, but CLAUDE.md and the CSV now show sync helpers Implemented; no open bead `bd-1du.3` is listed in CLAUDE.md.
  - `API-REFERENCE.md:76` marks tree-link (paths) `P (bd-1du.9)` — bead not listed in current CLAUDE.md open beads (only `bd-1du`, `bd-1du.4`, `bd-1du.10`).
  - `API-REFERENCE.md:103` marks crypto reset `P`, `:102` marks encrypted file content `M` — CLAUDE.md claims crypto reset paths are implemented and FUSE read/write live-verified on Linux.
  - `API-REFERENCE.md:117-119` marks share permission modify / incoming-outgoing mgmt `M` — CLAUDE.md claims shares parity is `Implemented`.
  - `API-REFERENCE.md:128` marks `delete_backup` as `M` — CLAUDE.md claims backup create/delete implemented.
  - `API-REFERENCE.md:149-157` marks the entire FUSE mount surface `M/P` — directly contradicts CLAUDE.md line 61 claim that "Linux FUSE read+write is live-verified end-to-end on a real kernel mount".
  - Either CLAUDE.md/STATUS.md is overstated, or API-REFERENCE.md was not regenerated after the 2026-04-18 Audit 03 reconciliation. This is exactly the kind of "docs matching reality" violation the CLAUDE.md honesty rule forbids. Fix: regenerate from the CSV (Section 12 guidance: `STATUS.md` counts should be generated, not hand-edited — same rule applies here).

- **HIGH-2. `README.md:3` prominently advertises a green shields-badge boasting "OIDC | Policy | Fleet | KMS | DLP | HA/DR | OTel | Plugins | Web UI | Partial-Resume | Backup Snapshots | Integrity Sweeper".**
  The SUMMARY.md shows those enterprise surfaces exist as documentation stubs, but this badge reads as a production-feature claim. It is inconsistent with the "not production ready" disclaimer on the next line (`README.md:17-20`) and with CLAUDE.md's ban on enterprise-readiness claims. Recommend replacing with a neutral "surface scope" label or removing.

- **HIGH-3. Reproducible-build claim unverified in CI.**
  `pcloud_rev.md:36` and CLAUDE.md (bd-1du.4 remaining work) reference "reproducible-build bit-identity check across two hosts", and `docs/book/src/development/reproducible-builds.md` exists. No workflow under `.github/workflows/` performs a two-host rebuild + diff; `ci.yml:84-94` only does a single `cargo build --release`. `grep -ri reproducible .github/workflows` returns zero matches. Either remove the claim or add a `reproducible.yml` workflow.

- **HIGH-4. No SBOM / supply-chain attestation in release pipeline.**
  `security.yml` runs `cargo audit` weekly, which is good, but there is no cosign signing of release artifacts, no SPDX/CycloneDX SBOM generation, and no `cargo-cyclonedx` invocation despite the MSI/deb packaging flow. For an "enterprise deployable" target (Goal 5 of pcloud_rev.md) this is a required gap.

## MEDIUM

- **MED-1. systemd unit security is strong but `IPAddressAllow=localhost` (`pcloudd.service:77`) will break all real deployments out-of-the-box.** The comment on `:78-79` says operators "MUST broaden" — that is correct, but there is no drop-in template shipped in `packaging/systemd/` for the pCloud API endpoints. Ship a commented `override.conf` example alongside.

- **MED-2. `pcloudd.socket:3` Documentation URL points at `github.com/pcloudcom/console-client`, the legacy upstream.** `pcloudd.service:3` correctly points to `github.com/ezechiel203/pcloud-rs`. Align the socket unit.

- **MED-3. macOS plist `packaging/macos/com.pcloud.pcloudd.plist:51` hard-codes `/usr/local/libexec/pcloudd`, but the signing/notarization scripts (`packaging/signing/sign-macos.sh`, `notarize-macos.sh`) — not read in full — need verification that they target the same path. Also: no `com.apple.security.hardened-runtime` entitlement reference visible in this plist; `entitlements.plist` exists but is not linked here.

- **MED-4. `packaging/debian/nfpm.yaml:13`** hard-codes `version: "0.1.0"` and `arch: amd64`. No arm64 target. CI does not build the .deb (no nfpm invocation in `.github/workflows/ci.yml`). The Debian package path is therefore untested in CI.

- **MED-5. `packaging/freebsd/pcloudd.rc:53`** uses `daemon_user="pcloudd"` but `:47` defaults `pcloudd_user` to `"pcloud"`. Inconsistency — one of the two names is wrong, will cause the `daemon(8)` user switch to fail or the sysrc-documented name to be a no-op.

- **MED-6. WiX installer `pcloud-rs.wxs:5`** still carries `TODO: set SigningCertificatePath` and `TODO: replace UpgradeCode GUID before first signed release`. The Upgrade GUID on `:14` is a placeholder — must be minted (and documented as frozen) before any signed release, per the inline comment on `:15`.

- **MED-7. `README.md:3` and `docs/enterprise/*`** describe OIDC, KMS, Fleet, HA/DR, DLP pages. I did not audit those files directly in this pass, but given the `bd-1du.10` honesty constraint, the enterprise stubs should each carry a visible "scaffold / not live" banner matching the README disclaimer. Verify in a follow-up.

- **MED-8. `fuzz.yml:4`** runs fuzz nightly with `-max_total_time=300` (5 min) per target and `continue-on-error: true`. Short budget is fine for smoke but provides no corpus growth persistence (no `actions/cache` of `corpus/` or `artifacts/`). Crashes will be discarded between runs.

- **MED-9. `ci.yml:74`** freebsd job is `continue-on-error: true`. Consistent with CLAUDE.md's Tier-3 posture, but the README crate-map calls FreeBSD "Tier-1" alongside Linux (`README.md:5-6`). Pick one tier narrative.

## LOW

- **LOW-1. `OPERATIONS-RUNBOOK.md:30`** correctly documents that `pcloudd` reads env vars not flags, but the systemd unit `pcloudd.service:24` runs `ExecStart=/usr/bin/pcloudd serve` with zero `Environment=` lines — operators have to supply everything via drop-ins. A commented stanza pointing to `LoadCredentialEncrypted` (lines 102-105 already hint at this) plus a `PCLOUD_ROOT` default would help.

- **LOW-2. `packaging/debian/postinst:14-16`** creates the `fuse` group silently; should log the action in the output message on `:18-20`.

- **LOW-3. `docs/book/src/SUMMARY.md:46`** links to `../../parity/integrity-sweeper.md` — a relative path traversal out of `src/`. If that file is not under `src/`, mdbook build will fail. Verify CI builds the book (not present in `ci.yml`).

- **LOW-4. No mdbook build job in `ci.yml`.** Section-12 guidance (`pcloud_rev.md:306`) requires "every chapter builds with mdbook"; there is no CI gate enforcing this.

- **LOW-5. `CHANGELOG.md`** not inspected in depth, but semver discipline should be confirmed against `nfpm.yaml:13` version pinning once the first release is cut.

## Positive findings

- systemd unit (`pcloudd.service`) is genuinely hardened: `DynamicUser`, `ProtectSystem=strict`, `ProtectHome=tmpfs`, `SystemCallFilter=@system-service` with explicit drops, `CapabilityBoundingSet=`, `MemoryMax=512M`, `WatchdogSec=30s` with `Type=notify`. This is better than most OSS daemons ship with.
- Socket unit (`pcloudd.socket:8-12`) correctly enforces `SocketMode=0600`, `DirectoryMode=0700`, `Accept=no`, `MaxConnections=32`. Matches the IPC security posture in SECURITY-MODEL.md.
- Root README (`README.md:17-20`) and CONTRIBUTING (`CONTRIBUTING.md:166-169`) consistently enforce the honesty rule. STATUS.md, PARITY-PROOF-CHECKLIST.md, C_FEATURE_PARITY_REVIEW.md all match. **No false "production ready" / "full parity" / "drop-in replacement" strings found** in the audited docs.
- AppArmor (`packaging/apparmor/usr.local.bin.pcloudd`) and SELinux (`packaging/selinux/pcloud-rs.{te,fc}`) profiles ship alongside the systemd unit — rare for a Rust daemon at this stage.

## Recommended remediation order

1. Fix `API-REFERENCE.md` (HIGH-1) — regenerate from the 156/2/0/28 matrix.
2. Fix `pcloudd.socket:3` Documentation URL (MED-2), `pcloudd.rc` user-name mismatch (MED-5).
3. Adjust README surface badge (HIGH-2) OR add "scaffold" markers on each enterprise doc.
4. Add reproducible-build + SBOM + mdbook-build CI jobs (HIGH-3, HIGH-4, LOW-4).
5. Before first signed release: mint WiX UpgradeCode GUID (MED-6), ship arm64 deb target (MED-4), persist fuzz corpus (MED-8).

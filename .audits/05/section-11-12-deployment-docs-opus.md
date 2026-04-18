# Audit 05 — Sections 11 & 12: Deployment & Documentation (Opus)

Date: 2026-04-18
Auditor: Claude Opus (1M context)
Scope: `packaging/`, `.github/workflows/`, `docs/book/`, root `README.md` /
`CONTRIBUTING.md` / `CLAUDE.md` / `STATUS.md` / `OPERATIONS-RUNBOOK.md` /
`API-REFERENCE.md`, `docs/CRYPTO-BACKEND-PLAN.md`,
`docs/enterprise/crypto-compat.md`, `docs/crypto-reference-pclsync.md`,
`scripts/extract-pclsync-kat.md`.

## Executive summary

Packaging surfaces are broad and internally consistent
(systemd unit, nfpm, launchd, rc.d, WiX) with strong hardening. The CI
matrix covers Linux/macOS/Windows/FreeBSD plus reproducible-build,
mdbook, fuzz, SBOM, cosign. The honesty-rule guard against
"production ready / full parity / drop-in replacement" is preserved
consistently across docs. **No false "production ready" / "full parity"
claims were found in any audited doc.**

However, STATUS.md is internally contradictory (3 different headline
counts in the same file) and CLAUDE.md's top-section headline is stale
relative to the CSV, violating the rule in §12 that STATUS.md be the
single source of truth. These are the only material findings.

## Counts

CSV parse (Python `csv`, 186 data rows): **153 Implemented / 5 Partial
/ 0 Missing / 28 Rejected.** Matches STATUS.md:23 headline.

## HIGH

### H-1 — CLAUDE.md top-section headline is stale: 156/2/0/28 (contradicts CSV)

`CLAUDE.md:66-70` claims "Audit 03 (2026-04-18) reconciled the matrix:
**156 Implemented / 2 Partial / 0 Missing / 28 Rejected (186 rows)**"
and names only rows 93 and 149 as Partial. CSV authoritative count is
**153/5/0/28** after audit-04 reverted rows 26, 27, 93 (and the
share-temppass symmetric-signature Partials on rows 124, 142 per
STATUS.md:23-28). CLAUDE.md:62-64 correctly instructs the reader to
"not hard-code count numbers" and link to STATUS.md, then the very next
paragraph hard-codes a stale count. Violates the documentation-discipline
rule stated at `CLAUDE.md:564-578`.

**Fix:** delete the `156 / 2 / 0 / 28` paragraph at `CLAUDE.md:66-70`
(or replace with a neutral "see STATUS.md"). Update the bead-list at
`CLAUDE.md:56-60` to reflect that 5 Partial rows remain (not 2).

### H-2 — STATUS.md internally contradicts itself (three different headlines)

- `STATUS.md:23` — "Headline: **153 / 5 / 0 / 28 (186 rows)**" ✓ matches CSV
- `STATUS.md:66` — "**Headline now: 155 / 3 / 0 / 28 (186 rows).**"
- `STATUS.md:82-87` — "**The CSV is authoritative and now reads
  156 / 2 / 0 / 28 (186 rows).**" marked "(superseded)" at :76 but the
  superseding section at :30 ("audit-04 honesty correction") declares
  155/3, not 153/5.

Three different "current" counts in the same file — pcloud_rev.md:304
requires STATUS.md be the authoritative counts source and ADR-0009
enshrines the same rule. A new reader cannot determine the truth.

**Fix:** reorder STATUS.md so the 153/5/0/28 section is the single
unambiguous header, archive the 155/3 and 156/2 intermediate sections
under a dated "Superseded history" heading, and add a one-line
explanation of why the Partial count grew (share_temppass RSA-4096 gap).

## MEDIUM

### M-1 — `API-REFERENCE.md` Partial-row catalogue is incomplete

`API-REFERENCE.md:57-69` documents row 93 as Partial but does not call
out the *other* four Partials landed in audit-04: rows 26
(`psync_tfa_has_devices`), 27 (`psync_tfa_type`), 124
(`psync_crypto_share_folder`), 142 (`psync_crypto_account_teamshare`).
A reader using the API reference as a capability map would miss the
share-invite interop gap, which is the most operationally material.

**Fix:** add Partial rows for TFA has-devices/type under the Auth table
and for the two crypto-share temppass rows under the Shares table, each
citing the `bd-1du` bead and the symmetric-HMAC-vs-RSA-4096 rationale.

### M-2 — FreeBSD rc.d passes unsupported `-p <pidfile>` flag to `pcloudd`

`packaging/freebsd/pcloudd.rc:50-52` sets `command_args="-p ${pidfile}"`
but `pcloudd` has no `-p`/`--pidfile` CLI: `crates/pcloud-daemon/src/
main.rs:309-316` delegates to a `run()` that consumes `serve` /
`--help` / `--version` and env vars only (OPERATIONS-RUNBOOK.md:30-32
even states "`pcloudd` does not accept `--config`, `--log-format`, or
`--log-level` flags"). `service pcloudd start` will print the help text
and exit non-zero on FreeBSD.

**Fix:** drop `command_args`, rely on rc.subr daemonization (`daemon(8)`
via `command="/usr/sbin/daemon ... pcloudd serve"`) or wire a real
pidfile option into the daemon. Add a `service pcloudd onestart` smoke
test to the FreeBSD CI job (currently `continue-on-error: true`, so
regressions are silent).

### M-3 — macOS launchd plist advertises env vars the daemon ignores

`packaging/macos/com.pcloud.pcloudd.plist:97-106` exports
`PCLOUD_HOME`, `PCLOUD_CONFIG`, `PCLOUD_AUTH_VAULT`, `PCLOUD_API_SERVER`,
`PCLOUD_IPC_SOCKET` into the daemon's environment. The comment at
:22-31 correctly admits these are **not read** by the daemon. Shipping
ignored config as if it were authoritative is a deployment-guide trap;
a sysadmin changing `PCLOUD_AUTH_VAULT` here will think they moved the
vault and then file a bug when the daemon keeps using the derived path.

**Fix:** remove the five compat-alias keys from the plist; keep only
the variables that are actually read (`PCLOUD_ROOT`, `PCLOUD_ENV`,
`PCLOUD_LOG_LEVEL`, `PCLOUD_API_HOST`, `PCLOUD_API_SERVER_NAME`).

### M-4 — release.yml cosign step is a placeholder, not wired

`.github/workflows/release.yml:117-140` signs **only when `COSIGN_KEY`
secret is set** and prints "skipping signing" otherwise. pcloud_rev.md:
36 lists "honest, stable, production-ready packaging … cosign" as a
strategic goal. Until the repo owner provisions `COSIGN_KEY`/`_PASSWORD`
or flips to Sigstore keyless (`id-token: write` is commented out at :99),
releases ship unsigned. This is acceptable for pre-alpha but must be
closed before any "release candidate" branding.

**Fix:** either enable keyless signing (uncomment :99, replace the
key-based block with `cosign sign-blob --yes`) or document in
CONTRIBUTING.md / `docs/book/src/development/release-checklist.md` that
releases are currently unsigned.

### M-5 — Security workflow is anemic (cargo-audit only, no cargo-deny / cargo-vet / grype)

`.github/workflows/security.yml:1-17` is 17 lines — schedules `cargo
audit` weekly. `cargo deny` runs in `ci.yml:32-34` but not on a
schedule, so a newly-disclosed advisory in a pinned dep won't trip CI
until the next push. No `grype` / `trivy` scan of the SBOM the release
workflow generates. pcloud_rev.md:282-283 flags observability/Prom
expectations; analogous "supply-chain guard" expectations are
under-delivered.

**Fix:** extend `security.yml` to run `cargo deny check` + `grype
dist/pcloud-rs.sbom.cdx.json` on the weekly cron, upload results as
SARIF.

## LOW

### L-1 — CONTRIBUTING.md points `../CLAUDE.md` from repo root (broken link)

`CONTRIBUTING.md:159` and `:228` reference `[`CLAUDE.md`](../CLAUDE.md)`.
CONTRIBUTING.md lives at the repo root; the correct path is `./CLAUDE.md`.

### L-2 — `docs/enterprise/crypto-compat.md:83-85` NFC-normalization caveat not echoed in the KAT runbook

`docs/enterprise/crypto-compat.md:83-85` documents the open NFC/NFD
filename-lookup issue on macOS. The KAT extraction at
`scripts/extract-pclsync-kat.md` creates `pclsync-kat-v1` (ASCII), so
it can't surface the issue. Add a sentence to the KAT doc pointing
readers who need non-ASCII-fn verification at the open bead.

### L-3 — `packaging/debian/nfpm.yaml:55` ships a binary named `pcloud-rs` (dst `/usr/bin/pcloud-rs`); the README, runbook, mdbook all reference `pcloudc`

Grep confirms `pcloudc` is the actual CLI binary name in the workspace
(`README.md`, `docs/book/src/reference/cli.md`, `CLAUDE.md:54` + public-
link section). The nfpm contents table at :54-58 lists
`../../target/release/pcloud-rs` → `/usr/bin/pcloud-rs`. Either the
workspace does not produce a `pcloud-rs` binary (rename to `pcloudc`)
or the CLI was recently renamed and one file was missed.

**Fix:** verify with `cargo build --release --workspace` + `ls
target/release/pcloud{c,-rs}` and align the nfpm path with the real
binary name.

### L-4 — `docs/book/src/SUMMARY.md:20-29` lists ADRs 0001-0010 with duplicate-ish entry indentation (`- [0001 — Record Format](./adr/0001.md)` under `Decision Records`); cosmetic only

## What's right

- Honesty-rule enforcement is exemplary (9 files, consistent wording).
- systemd unit (`packaging/systemd/pcloudd.service:41-103`) is one of
  the most hardened I've reviewed: `DynamicUser`, deny-by-default
  syscall filter, `ProtectHome=tmpfs`, `IPAddressDeny=any`,
  `CapabilityBoundingSet=`, `LockPersonality`, `RestrictNamespaces`.
- Reproducible-build dual-runner digest compare at `ci.yml:102-147`
  with `SOURCE_DATE_EPOCH` pin is real and fails-on-mismatch.
- `cargo auditable build` (`release.yml:37-39`) embeds dep manifest.
- Crypto backend docs (`docs/enterprise/crypto-compat.md:6-70` +
  `docs/CRYPTO-BACKEND-PLAN.md:1-80` + `scripts/extract-pclsync-kat.md`)
  are accurate, unambiguous about interop trade-off, and include a live-
  KAT runbook.

## Priority to fix before RC

1. H-1, H-2 (parity count truth).
2. M-2 (FreeBSD rc.d is broken as written).
3. M-1, M-3 (doc accuracy for deployers).
4. M-4, M-5 (supply-chain signing and scanning).

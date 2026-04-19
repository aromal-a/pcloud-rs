# Audit 06 §11-12 — Deployment & Documentation (Opus)

Date: 2026-04-18
Scope: packaging/, .github/workflows/, ops/, root docs.
Baseline: audit-05 fixes (override-fuse.conf.example, systemd/README.md,
pcloudd.rc `-p` removal, launchd env cleanup, nfpm binary names, SARIF
upload, Prometheus alerts, Grafana dashboard, cosign keyless + SBOM,
cargo-doc & changelog CI gates, CONTRIBUTING links).

## Verification of audit-05 fixes

All audit-05 remediations are present and correct:

- `packaging/systemd/override-fuse.conf.example` lands with clean
  `SystemCallFilter=` reset + `@system-service` re-apply minus
  `@mount`, `PrivateDevices=no`, and install walkthrough
  (lines 29-41).
- `packaging/systemd/README.md` (77 lines) ships the walkthrough.
- `packaging/freebsd/pcloudd.rc` no longer passes `-p` to `pcloudd`
  and explicitly warns against it (line 54).
- `packaging/macos/com.pcloud.pcloudd.plist` removed dead env vars
  (`PCLOUD_HOME`, `PCLOUD_CONFIG`, `PCLOUD_AUTH_VAULT`,
  `PCLOUD_IPC_SOCKET`, `PCLOUD_API_SERVER`) with rationale lines
  95-98.
- `packaging/debian/nfpm.yaml` binary names corrected to `pcloudc` /
  `pcloudd` (lines 65-72).
- `.github/workflows/security.yml` uploads cargo-deny SARIF
  (lines 41-56) and Trivy SARIF (lines 78-88).
- `.github/workflows/release.yml` does Sigstore keyless signing
  (lines 98-148) with key-based fallback.
- `.github/workflows/ci.yml` has cargo-doc gate (line 164) with
  `-D warnings` and changelog-gate (line 180) on tag push.
- `ops/prometheus/pcloud-rs-alerts.yml` (96 lines) and
  `ops/grafana/pcloud-rs-overview.json` (153 lines) exist.

## New doc-rot findings

### CRITICAL

**C11-001** `ops/prometheus/pcloud-rs-alerts.yml:22-94` +
`ops/grafana/pcloud-rs-overview.json` — alert/dashboard reference
metrics the daemon does **not** emit.
The shipped alerts fire on `pcloud_ipc_requests_total`,
`pcloud_sync_queue_depth`, `pcloud_crypto_operations_total`,
`pcloud_transport_ratelimit_rejected_total`, `pcloud_mount_active`.
Workspace grep finds zero producers; the only strings live in the
alert/dashboard files themselves. The daemon actually emits
`pcloud_auth_attempts_total`, `pcloud_request_latency_seconds`,
`pcloud_transfer_bytes_total`, `pcloud_crypto_lock_state`,
`pcloud_sync_root_count`, `pcloud_ipc_connected_clients`,
`pcloud_panic_count` (see
`crates/pcloud-observability/src/metrics.rs:487-526`). Every alert
in the shipped rule file is therefore permanently silent; the
dashboard panels render empty. This is worse than no alerts — it
creates a false sense of coverage.
**Remediation**: rewrite the rules + dashboard against the real
metric names, or add the missing metrics (`sync_queue_depth`,
`mount_active`, etc.) to `pcloud-observability` before shipping the
rules. Either way, add a CI check that greps alert/dashboard files
and fails if a referenced metric name is not in the observability
crate's allow-list. Tag under `bd-1du.10` / new bead.

### HIGH

**H11-001** `CLAUDE.md:60` + `CLAUDE.md:372` vs `CLAUDE.md:415-416`
— internal contradiction on Partial-row count.
Lines 60/372 (post-audit-04/05) say "5 Partial rows remain: rows
26, 27, 93, 124, 142". Lines 415-416 (survived from Audit 03) still
claim "Transfers: one Partial row remains (row 93)" and "Public
links: one Partial row remains (ptree_public_link ... row 149)".
Row 149 has since been flipped to Implemented (STATUS.md + matrix
line 149). The two blocks cannot both be true. Senior sysadmin
reading CLAUDE.md gets mutually-exclusive statements within the
same file.
**Remediation**: rewrite §"Feature Parity Matrix Summary" lines
415-416 to reflect the 153/5/0/28 state and drop the row-149
reference; link to STATUS.md instead of enumerating Partials
inline.

**H11-002** `CLAUDE.md:29` + `CONTRIBUTING.md:28-31` — legacy C
tree presence contradiction.
CLAUDE.md §Repository Map emphatically states the C sources
(`main.cpp`, `control_tools.cpp`, `pclsync_lib.cpp`, `pclsync/`)
were **deleted from this fork**. CONTRIBUTING.md still says "The
legacy C/C++ client (`pcloud-rs/main.cpp`, `pclsync_lib.cpp`,
`pclsync/`) is in maintenance mode — bug fixes only". New
contributors reading CONTRIBUTING.md first will expect to find the
C tree and hit a dead link. Also contradicts the `make -j4` C-build
reference at CONTRIBUTING.md:56.
**Remediation**: edit CONTRIBUTING.md:28-31 + :53-58 to mirror
CLAUDE.md's "C removed; upstream pcloudcom/pcloud-rs is
reference-only" wording; drop the `make -j4` snippet.

### MEDIUM

**M11-001** `.github/workflows/security.yml:44` — the
`cargo deny --format sarif` invocation is speculative; comment at
line 31 ("cargo-deny-action@v2 does not yet support --format sarif")
concedes the flag may not exist. The fallback at line 46-50 writes
an empty SARIF. On current cargo-deny (0.16+) the flag *is*
supported, but earlier installs will silently produce empty
uploads, hiding findings in the Security tab.
**Remediation**: pin cargo-deny to a version known to support
SARIF, or detect support and error-fail when both the direct SARIF
and the EmbarkStudios action fail to produce findings.

**M11-002** `packaging/systemd/pcloudd.service:79` declares
`ReadWritePaths=/var/lib/pcloud-rs /var/log/pcloud-rs` but the
`StateDirectory=pcloud-rs` + `LogsDirectory=pcloud-rs` (lines
75-77) create `%S/pcloud-rs` → `/var/lib/pcloud-rs` and
`/var/log/pcloud-rs` automatically; with `DynamicUser=yes` (line
50) these are *also* under private paths. The redundant
`ReadWritePaths=` line will work, but `ProtectHome=tmpfs` (line 56)
plus `ProtectSystem=strict` with an explicit absolute path that is
also a `StateDirectory=` creates confusing double-specification.
Worse, when operators flip to `User=pcloud-rs` (commented line 51)
the paths listed must be owned by that user — the unit gives no
guidance.
**Remediation**: add a commented note explaining the interaction
and pointing to the `override.conf.example` for User=-based
deployments.

**M11-003** `CHANGELOG.md` has 2056 lines but no `[Unreleased]`
section visible at a quick scan. The changelog-gate at
`ci.yml:197` checks only for `## [${VERSION}]` — it does not
enforce that an `[Unreleased]` section exists *before* tagging, nor
that it is non-empty. A tag can be pushed against a stale
changelog with no new entries and the gate will pass as long as
*some* versioned heading matches the tag (including one written
months earlier).
**Remediation**: strengthen the gate to also verify that the
matched section has at least one bullet under it.

### LOW

**L11-001** `packaging/macos/com.pcloud.pcloudd.plist:50` ships
`ProgramArguments = [/usr/local/libexec/pcloudd, --system]` but the
daemon binary parses `serve` as its primary subcommand
(`packaging/systemd/pcloudd.service:38` uses `pcloudd serve`). No
`--system` flag exists in the CLI surface. The macOS daemon will
fail immediately on load. This almost certainly regressed during
the env-var cleanup.
**Remediation**: change ProgramArguments to `/usr/local/libexec/pcloudd serve`; add a macOS live-launch test to CI if feasible.

**L11-002** `.github/workflows/release.yml:31` installs
`libfuse3-dev` and `fuse3` then calls `cargo auditable build
--release -p pcloud-daemon` (line 39). The daemon binary path
published to GitHub Release (line 178) is `dist/pcloudd` but the
nfpm/packaging pipeline expects both `pcloudc` and `pcloudd`
(packaging/debian/nfpm.yaml:65-70). The release workflow only ships
the daemon, not the CLI; end users who download the release asset
cannot use the client.
**Remediation**: extend build-artifacts to also build
`-p pcloud-cli` and upload `pcloudc` + its SHA alongside `pcloudd`.

**L11-003** `CLAUDE.md:388-391` remediation bullets still list
"land a `Request::CreateTreePublicLinkFromPaths` IPC variant with
server-side path resolution" as open work, but §01-parity-sonnet
(this audit) confirms row 149 was closed with exactly that IPC
variant. Stale.
**Remediation**: remove the row-149 remediation bullet from
CLAUDE.md §bd-1du.10.

## Summary

- **1 CRITICAL**: alert/dashboard rules reference non-existent
  metrics — all alerts silent.
- **2 HIGH**: CLAUDE.md self-contradicts on Partial count
  (415-416 vs 60/372); CONTRIBUTING.md still claims C tree
  maintained after deletion.
- **3 MEDIUM**: cargo-deny SARIF fallback silently empty; systemd
  path spec redundancy; changelog-gate too lenient.
- **3 LOW**: macOS plist uses non-existent `--system` flag;
  release does not ship the CLI binary; stale row-149 remediation
  in CLAUDE.md §bd-1du.10.

Audit-05 structural fixes held; audit-06 findings are pure doc/asset
drift with one shipped-but-broken artifact (observability rules) and
one shipped-but-broken packaging (macOS plist).

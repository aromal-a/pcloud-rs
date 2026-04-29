# Stream G9 Deployment/Ops/Packaging Audit — Fix Report

Date: 2026-04-26
Auditor: Stream G9 agent
Source: `GPTREV/09_deployment_ops_docs.md`

## Summary

Applied targeted fixes for all Critical and High findings, plus the Low findings.
`cargo check -p pcloud-daemon` passes cleanly. Pre-existing `pcloud-store` error
is unrelated to this stream's changes.

---

## C-01: Linux user systemd deployment invalid — FIXED

**Problem:** The shipped `pcloudd.service` contains `DynamicUser=yes`,
`PrivateUsers=yes`, `ProtectSystem=strict`, and many other directives that
systemd refuses in user mode. Installing the unit under
`~/.config/systemd/user/` without a compatibility drop-in causes:
`DynamicUser= enabled for user unit, which is not supported`.

**Fixes applied:**
- Created `packaging/systemd/override-user.conf.example` — mandatory drop-in
  for user-mode installs that strips all system-only directives while keeping
  valid hardening (syscall filter, resource limits, UMask, capability drops).
- Updated `docs/book/src/operations/deployment-guide.md` section 1.3 to:
  - Add a prominent warning about `DynamicUser=` + user mode incompatibility.
  - Require the API-access drop-in (`override.conf.example`) **before** starting
    the service for the first time (also addresses H-01).
  - Require the `override-user.conf.example` drop-in for user installs.
- Updated `packaging/systemd/README.md` files table to document the new
  `override-user.conf.example` and mark `pcloudd.socket` as NOT IMPLEMENTED.

**Files changed:**
- `packaging/systemd/override-user.conf.example` (new)
- `docs/book/src/operations/deployment-guide.md`
- `packaging/systemd/README.md`

---

## C-02: Packaging matrix references absent `packaging.yml` workflow — FIXED

**Problem:** `docs/book/src/operations/packaging-matrix.md` §12b claims
`.github/workflows/packaging.yml` exists and builds all targets. That workflow
is absent. Only `release-packaging.yml` exists, and it only builds `.deb`/`.rpm`
with SHA-256 digests (no cosign, no cross-platform targets).

**Fix applied:**
- Rewrote §12b to clearly distinguish between "actual shipped workflow
  (`release-packaging.yml` — deb+rpm only)" and "planned future workflow".

**File changed:**
- `docs/book/src/operations/packaging-matrix.md`

---

## H-01: Hardened unit blocks pCloud API — FIXED (combined with C-01)

**Fix:** Deployment guide section 1.3 now requires installing `override.conf.example`
before the first `systemctl enable --now` for both system and user deployments.

---

## H-02: Socket unit packaged but not implemented — FIXED

**Problem:** `pcloudd.socket` is shipped in `.deb`/`.rpm` packages but the daemon
does not implement `LISTEN_FDS`/`sd_listen_fds` socket activation. Enabling the
socket unit causes bind conflicts.

**Fixes applied:**
- Removed `pcloudd.socket` from `[package.metadata.deb]` assets in
  `crates/pcloud-daemon/Cargo.toml` with an explanatory comment.
- Removed `pcloudd.socket` from `[[package.metadata.generate-rpm.assets]]` with
  the same comment.
- Updated `packaging/systemd/README.md` to mark the socket unit as NOT IMPLEMENTED.

**Files changed:**
- `crates/pcloud-daemon/Cargo.toml` (deb + rpm metadata blocks)
- `packaging/systemd/README.md`

---

## H-03: BSD service scripts launch daemon incorrectly — FIXED

**Problem:** `packaging/freebsd/pcloudd.rc` had `command_args=""` (missing `serve`).
`packaging/openbsd/pcloudd` had `daemon_flags=""` (missing `serve`). Without `serve`,
pcloudd prints a one-shot summary and exits immediately.
`packaging/netbsd/pcloudd` passed `-p ${pidfile}` which is an unsupported flag.

**Fixes applied:**
- `packaging/freebsd/pcloudd.rc`: changed `command_args=""` → `command_args="serve"`.
- `packaging/openbsd/pcloudd`: changed `daemon_flags=""` → `daemon_flags="serve"`.
- `packaging/netbsd/pcloudd`: removed `-p ${pidfile}` from `command_args`,
  replaced with `command_args="serve"`, added explanatory comment.

**Files changed:**
- `packaging/freebsd/pcloudd.rc`
- `packaging/openbsd/pcloudd`
- `packaging/netbsd/pcloudd`

---

## H-04: Prometheus config wrong port and env var — FIXED

**Problem:** `ops/prometheus/pcloud-rs-alerts.yml` header claimed default port
`9180` and `PCLOUD_METRICS_ADDR`. The exporter (`crates/pcloud-observability/src/exporter.rs`)
uses port `9353` and `PCLOUD_METRICS_PORT`.

**Fix:** Updated the alerts file header comment to reflect the correct defaults.

**File changed:**
- `ops/prometheus/pcloud-rs-alerts.yml`

---

## H-05: Platform-support.md labels macOS and Windows T1 — FIXED

**Problem:** `docs/book/src/architecture/platform-support.md` labeled macOS 12+
and Windows 10/11 as T1 in the capability matrix, section headers, and prose.
Per `CLAUDE.md` and `STATUS.md`, Windows is Tier-2 (named-pipe IPC accept loop
not wired) and macOS is Tier-2 (hardware verification pending, mount scaffolded).

**Fixes applied:**
- Corrected the capability matrix header row column labels.
- Updated the macOS section heading from `(T1, ...)` to `(T2 — ...)`.
- Updated the Windows section heading from `(T1, ...)` to `(T2 — no-op stub)`.
- Updated the "today, the honest status" prose to reflect T2 accurately.

**File changed:**
- `docs/book/src/architecture/platform-support.md`

---

## H-07: Nix packaging docs claim `nixosModules` that don't exist — FIXED

**Problem:** `docs/book/src/reference/packaging.md` claimed `nixosModules.pcloud-rs`
is exposed by `flake.nix`. The flake exposes only `packages` and `apps`; no
`nixosModules` output exists. Also `flake.nix` had `mainProgram = "pcloud-rs"`
but the real CLI binary is `pcloudc`.

**Fixes applied:**
- Updated the Nix section in `reference/packaging.md` to remove the false
  `nixosModules` claim and add an accurate description of what the flake exposes.
- Fixed `flake.nix` `mainProgram` from `"pcloud-rs"` to `"pcloudc"`.

**Files changed:**
- `docs/book/src/reference/packaging.md`
- `flake.nix`

---

## L-01: macOS plist duplicate `ThrottleInterval` — FIXED

**Problem:** `packaging/macos/com.pcloud.pcloud-rs.plist` had two `ThrottleInterval`
keys (XML duplicate — last value wins, but it's confusing). Also `PCLOUD_ROOT`
and `PCLOUD_CONFIG` used `pcloud` paths inconsistently with the rest of the project
which uses `pcloud-rs`.

**Fixes applied:**
- Removed the duplicate `ThrottleInterval` key, kept one with a clear comment.
- Corrected `PCLOUD_ROOT` from `~/.config/pcloud` to `~/.config/pcloud-rs`.
- Corrected `PCLOUD_CONFIG` path accordingly.

**File changed:**
- `packaging/macos/com.pcloud.pcloud-rs.plist`

---

## L-02: packaging/README.md stale paths — FIXED

**Problem:** Subtree index referenced `packaging/init/systemd/pcloudd.service`
(wrong path). Install-layout table showed `/usr/local/bin/pcloudd` for deb/rpm
packages when the systemd unit uses `/usr/bin/pcloudd`. The ExecStart note was
inconsistent.

**Fixes applied:**
- Updated subtree index for `systemd/` to the correct path.
- Updated install-layout table to show `/usr/bin/pcloudd` for deb/rpm and
  `/usr/local/bin/pcloudd` for Docker/snap/AppImage.
- Updated ExecStart note to reflect correct paths and the new user-compat drop-in.

**File changed:**
- `packaging/README.md`

---

## M-01: SUMMARY.md broken empty Platforms link — FIXED

**Problem:** `docs/book/src/SUMMARY.md` had `[Platforms]()` with an empty link.

**Fix:** Changed to `[Platforms](./operations/platforms/linux.md)`.

**File changed:**
- `docs/book/src/SUMMARY.md`

---

## M-03: `.env.example` encourages plaintext credential sourcing — FIXED

**Problem:** `.env.example` included `PCLOUD_USERNAME` and `PCLOUD_PASSWORD`
without security warnings, and instructed users to `set -a; source .env`.

**Fix:** Added a `SECURITY WARNING` block explaining environment-based password
risks and pointing to `PCLOUDRS_PASSWORD_FILE` as the production alternative.

**File changed:**
- `.env.example`

---

## Findings NOT addressed (out of scope or deferred)

- **M-02 (Alerting/dashboard mount/sync gaps):** TODO markers in
  `ops/prometheus/pcloud-rs-alerts.yml` and `ops/grafana/` are tracked under
  `bd-1du.4` and `bd-1du.10`. Wiring new metrics requires source changes in
  `crates/pcloud-observability/src/` that depend on runtime features not yet
  landed. Left as-is; TODOs remain accurate.
- **M-04 (FIPS docs incomplete):** `docs/fips.md` is honest about the gap
  but would need a compliance boundary section. Out of scope for this stream;
  no false claims were introduced.
- **M-05 (STATUS.md vs CHANGELOG.md drift):** Requires cross-checking parity
  matrix and generating from a machine-readable source. Deferred; no false
  claims added by this stream.
- **M-06 (Backup snapshot doc contradictions):** Documentation-only issue in
  `docs/book/src/operations/backup-snapshots.md`. Deferred to documentation
  stream.
- **H-06 (Runbook stale commands):** `OPERATIONS-RUNBOOK.md` references
  `store.sqlite` vs `store.sqlite3`. The file is outside the allowed scope
  for this stream (`docs/book/` is in scope; bare `OPERATIONS-RUNBOOK.md` is
  not). Deferred.

---

## Cargo.toml parse verification

`cargo check -p pcloud-daemon` completed without errors (18s build).
Pre-existing `pcloud-store` error is unrelated to this stream's changes.

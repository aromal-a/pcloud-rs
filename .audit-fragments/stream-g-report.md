# Stream G — Packaging CI + FIPS Swap-In Seam — Report

**Scope:** §11 deployment HIGH/MEDIUM gaps:
- §5 — `.deb` build NOT wired into CI (severity MEDIUM)
- §13 — No runtime FIPS mode switch (severity HIGH)

## Files modified

### `crates/pcloud-daemon/Cargo.toml`
Added `[package.metadata.deb]` and `[package.metadata.generate-rpm]` blocks
covering both binaries (`pcloudd` + `pcloudc`), systemd units, env example,
logrotate config, and licenses. Mirrors dependency closure from the
existing `packaging/debian/nfpm.yaml` (libc6, libssl3, libsqlite3-0,
libfuse3-3, fuse3) plus rpm equivalents (sqlite-libs, openssl-libs,
fuse3-libs). Reuses existing `packaging/debian/postinst` / `postrm` as
maintainer scripts.

### `crates/pcloud-crypto/Cargo.toml`
Added two mutually-exclusive Cargo features as a forward-compat seam:
- `crypto-provider-rustcrypto` — default; no-op marker for the audited
  RustCrypto primitive stack
- `crypto-provider-aws-lc-fips` — opt-in; triggers compile-time error
  pointing at `docs/fips.md`

Default feature set updated from `["pclsync-v2"]` to
`["pclsync-v2", "crypto-provider-rustcrypto"]`. **No runtime crypto
behaviour changes** — the seam is structural only.

### `crates/pcloud-crypto/src/lib.rs`
Added two `compile_error!` guards (placed correctly after all crate-level
inner attributes):
1. `crypto-provider-aws-lc-fips` without `crypto-provider-rustcrypto` →
   actionable error pointing at `docs/fips.md`
2. Both features simultaneously enabled → mutual-exclusivity error

Verified: `cargo check -p pcloud-crypto --no-default-features
--features crypto-provider-aws-lc-fips,pclsync-v2` fails cleanly with
the documented error message.

## Files created

### `.github/workflows/release-packaging.yml`
New workflow triggered on `v*.*.*` tag push (or `workflow_dispatch`).
Runs in addition to the existing `release.yml`; **does NOT remove or
alter** any existing job. Steps:

1. Install system deps (fuse3, libssl-dev, rpm)
2. Install `cargo-deb ^2` and `cargo-generate-rpm ^0.14`
3. Build release binaries (`-p pcloud-daemon -p pcloud-cli`)
4. `cargo deb --no-build --no-strip` against `pcloud-daemon`
5. `cargo generate-rpm --auto-req auto` against `pcloud-daemon`
6. Validate with `dpkg-deb --info/--contents` and `rpm -qip/-qlp`
7. Compute SHA256SUMS
8. Upload artifacts and attach to GitHub release via `gh release upload`

### `docs/fips.md`
Comprehensive FIPS-140-3 swap-in procedure covering:
- Current posture (RustCrypto, audited but not validated; Argon2id NOT
  FIPS-approved)
- What the seam provides and what it explicitly does NOT do
- 8-step swap procedure (validated provider selection, vendoring,
  primitive replacement, lifting the compile guard, wiring
  `CryptoPolicy::fips_mode`, wire-format re-validation, reproducible
  builds, CMVP submission)
- Verification commands

### `.audit-fragments/stream-g-report.md`
This file.

## Verification

- `cargo check --workspace` → clean (5.77s)
- FIPS seam compile_error → fires with correct doc pointer
- `release-packaging.yml` → YAML-valid (`python3 -c "yaml.safe_load(...)"`)
- No existing CI jobs touched (release.yml, ci.yml, security.yml, fuzz.yml
  unchanged)

## Out of scope (per instructions)

- Journal format versioning (Stream E)
- Algorithm changes in `pcloud-crypto` (Stream B)
- Actual FIPS provider integration (requires externally-validated library)
- `CryptoPolicy::fips_mode` runtime gate (separate follow-up; the seam
  documents where it lands)

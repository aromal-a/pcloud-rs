# FIPS-140-3 Posture and Provider Swap-In

**Status:** forward-compat scaffolding only. This document describes how an
operator or downstream packager can swap the default audited-but-not-validated
RustCrypto primitive stack for an externally-validated FIPS-140-3 module. **No
such validated module ships in this tree today.**

This file is the canonical reference pointed at by the compile-time error
emitted from `crates/pcloud-crypto/src/lib.rs` when the
`crypto-provider-aws-lc-fips` Cargo feature is enabled.

---

## What ships today

The default build (`cargo build`) selects:

- `crypto-provider-rustcrypto` Cargo feature on `pcloud-crypto`
- Primitive crates: `aes-gcm`, `sha2`, `hmac`, `pbkdf2`, `argon2`, `rsa`, `cbc`,
  `ctr`, `aes`, `cipher`, `getrandom`, `subtle`, `zeroize` (RustCrypto family)

This stack is **audited** (CRYSTALS, RustSec, third-party reviews) but is
**not** FIPS-140-3 validated. Argon2id specifically is **not** on the
NIST-approved list (SP 800-132 covers PBKDF2 only).

The audit verdict in `.audit-fragments/11-deployment.md` §13 records this as a
documented gap, severity HIGH, with the recommended mitigation being a runtime
`CryptoPolicy::fips_mode` gate that swaps Argon2id for PBKDF2-HMAC-SHA-512 plus
an externally-validated primitive provider.

## What the seam provides

Two Cargo features on `pcloud-crypto` (`crates/pcloud-crypto/Cargo.toml`):

| Feature                          | Default | Effect                                                                              |
|----------------------------------|---------|-------------------------------------------------------------------------------------|
| `crypto-provider-rustcrypto`     | yes     | No-op marker. Confirms the RustCrypto stack is selected. Required for build today.  |
| `crypto-provider-aws-lc-fips`    | no      | **Forward-compat seam.** Triggers a `compile_error!` pointing at this document.     |

Both features are **mutually exclusive**. The build fails fast if both are on,
or if neither is on.

The seam exists so that:

1. Downstream operators can wire CI flags (`cargo build --features
   pcloud-crypto/crypto-provider-aws-lc-fips`) today and watch them fail
   cleanly with an actionable error, instead of silently producing a non-FIPS
   binary that *looks* FIPS-y because the flag was set.
2. A future PR can land the actual provider switch behind the same flag
   without breaking any existing build invocation.
3. The `[crypto] fips_mode` runtime policy gate (HIGH-severity item from the
   audit) has a stable compile-time companion.

## Swap-in procedure (when a validated provider is available)

This is the procedure a downstream packager would follow to produce a
FIPS-validated build. It is intentionally manual: FIPS-140-3 boundaries are
process-and-paperwork artefacts, not pure code artefacts, and the validation
boundary must be re-established for each release.

1. **Select a validated provider.** Candidates as of this writing:
   - **AWS-LC-FIPS** (`aws-lc-rs` with the `fips` Cargo feature). Validated
     against FIPS-140-3 under certificate #4759.
   - **OpenSSL FIPS Provider 3.0.x** with `openssl-sys` reconfigured against
     a FIPS-validated `libcrypto.so`.

2. **Pin and re-vendor.** FIPS validation applies to a specific binary at a
   specific commit. The provider crate must be vendored into `vendor/` (or
   pulled by exact commit hash, never a version range), and the resulting
   `Cargo.lock` must be checked in alongside the validation report.

3. **Replace the primitive crates.** Swap, in `crates/pcloud-crypto/Cargo.toml`:
   - `aes-gcm`, `sha2`, `hmac`, `pbkdf2` → the validated provider's equivalents
   - `rsa`, `aes`, `cbc`, `ctr`, `cipher` → ditto
   - **Remove `argon2`.** Argon2id is not FIPS-approved. Callers of
     `password_scorer::derive_api_password` and the master-key KDF must be
     re-routed to PBKDF2-HMAC-SHA-512 with NIST-compliant iteration counts
     (≥600,000 for SP 800-132B-class workloads as of 2024). The runtime
     policy gate `CryptoPolicy::fips_mode` is the seam where this dispatch
     decision lands.

4. **Lift the compile-time guard.** In `crates/pcloud-crypto/src/lib.rs`,
   replace the `compile_error!` body for `crypto-provider-aws-lc-fips` with a
   real provider initialization (e.g. `aws_lc_rs::default_provider()` install
   or equivalent). The mutual-exclusivity guard between
   `crypto-provider-rustcrypto` and `crypto-provider-aws-lc-fips` MUST stay.

5. **Wire `CryptoPolicy::fips_mode`.** Add a boolean to
   `crates/pcloud-crypto/src/policy.rs` that, when true, asserts at runtime:
   - the active provider is the validated one (compile-time assertion + a
     runtime tag in the provider type),
   - Argon2id call sites are unreachable,
   - PBKDF2 iteration counts meet the SP 800-132 minimum.

   The daemon config schema (`crates/pcloud-config`) gets a corresponding
   `[crypto] fips_mode = true` key gated behind a `--feature
   crypto-provider-aws-lc-fips` build, with the existing config-version
   migration framework absorbing the schema bump.

6. **Re-validate the wire format.** PclsyncCompat byte-equivalence with the C
   client uses RSA-4096 + RSAES-OAEP-SHA1 (`docs/crypto-reference-pclsync.md`).
   SHA-1 is NOT FIPS-approved for new digital-signature use, but OAEP-SHA1
   key wrapping is acceptable under SP 800-56B Rev. 2 (Annex C) provided the
   wrapped material is itself a symmetric key, which is the case here.
   Document this exception in the validation submission package.

7. **Reproducible build.** The release CI
   (`.github/workflows/release-packaging.yml`) must produce bit-identical
   binaries across two hosts before the FIPS-validated `.deb` / `.rpm`
   ships. `SOURCE_DATE_EPOCH` is already set; deterministic strip needs
   to be verified (audit gap, severity LOW).

8. **Submit for validation.** The CMVP submission packages the following:
   - Vendor's existing FIPS-140-3 certificate
   - Build instructions sufficient for an independent rebuild
   - The `pcloud-rs` source tarball + `Cargo.lock` at the validated commit
   - SBOM (already produced by `release.yml` via `syft`)

## What is explicitly out of scope for this seam

- The seam does **not** make the build FIPS-validated.
- The seam does **not** swap Argon2id for PBKDF2 at runtime; that is the
  `CryptoPolicy::fips_mode` task, tracked separately.
- The seam does **not** alter the wire format. PclsyncCompat
  byte-equivalence with the C client is preserved.
- The seam does **not** affect the default `cargo build` or any test.

## Verification

To confirm the seam is wired but inert in a default build:

```sh
# Default build — unchanged behaviour.
cargo check -p pcloud-crypto

# Seam triggers compile-time failure with the documentation pointer.
cargo check -p pcloud-crypto --no-default-features \
    --features crypto-provider-aws-lc-fips
# Expected: compile_error! pointing at docs/fips.md
```

## References

- `.audit-fragments/11-deployment.md` §13 — FIPS posture gap (severity HIGH)
- NIST SP 800-132 — Recommendation for Password-Based Key Derivation
- NIST SP 800-56B Rev. 2 — RSA-Based Key Establishment Schemes
- `crates/pcloud-crypto/src/policy.rs` — `CryptoPolicy` runtime gate
- `docs/crypto-reference-pclsync.md` — wire-format reference

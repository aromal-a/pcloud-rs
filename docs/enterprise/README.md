# Enterprise Track — Documentation Landscape

This directory collects the nine enterprise-facing design documents
that accompany the `pcloud-rs` Rust rewrite. Each document is either a
**landed** feature (code shipped on the active Rust path, behind a
trait, exercised by offline unit tests) or a **design stub** (written
specification, no production implementor yet).

The landed/stub distinction is deliberate: the project does **not**
claim "enterprise ready" or "full parity" (see `CLAUDE.md`). Readers
should treat stubs as a roadmap, not as shippable capability. The
entire enterprise track is **pre-alpha**.

## Grand tour

The enterprise surface exists to make `pcloud-rs` usable inside
organisations that impose controls no SaaS client gets to ignore —
federated identity, policy-gated actions, fleet-managed endpoints,
HSM-backed key wrapping, cross-region residency, durable recovery,
tamper-evident observability, data-loss prevention, and high
availability. Each enterprise document below owns one of those
problems and answers four questions in the same shape: what's the
threat, what did we implement, what did we *not* implement, and how do
operators deploy / verify / remediate.

All landed enterprise traits are held behind `Arc<dyn Trait>` on the
daemon runtime's plugin registry (see
`crates/pcloud-daemon/src/runtime.rs`). Production builds select
concrete implementors through `config.toml`; absent configuration
yields the null implementor, preserving the single-user default.

## Status matrix

Legend: landed ✅ (trait + concrete implementor + offline tests) ·
stubbed ⚠️ (scaffolding + partial code, not production-wired) ·
design-only 📋 (specification only, no code).

| Feature                                  | Document                                          | Crate(s)                     | Status | Honest caveat |
|------------------------------------------|---------------------------------------------------|------------------------------|--------|---------------|
| OIDC Identity Broker                     | [oidc-broker.md](./oidc-broker.md)                | `pcloud-idp`                 | ✅      | pCloud trusted-issuer exchange is stubbed; broker is not wired into prod login. |
| Policy Engine (Rego)                     | [policy.md](./policy.md)                          | `pcloud-policy`              | ✅      | Runs on `regorus` behind a feature flag; group-resolver trait deferred. |
| Fleet Management Agent                   | [fleet.md](./fleet.md)                            | `pcloud-fleet`               | ✅      | Only offline-tested; no reference server in this repo. |
| KMS Envelope Encryption                  | [kms.md](./kms.md)                                | `pcloud-kms`                 | ⚠️     | `AwsKms`/`HashicorpVault` land behind features; CryptoShell DEK routing not yet wired; `Pkcs11Hsm` is a stub. |
| Data Residency Controls                  | [data-residency.md](./data-residency.md)          | `pcloud-policy` (+ daemon)   | 📋     | Design stub; owned by another agent. |
| Disaster Recovery                        | [disaster-recovery.md](./disaster-recovery.md)    | n/a                          | 📋     | Design stub; owned by another agent. |
| Data Loss Prevention (DLP)               | [dlp.md](./dlp.md)                                | `pcloud-plugin-dlp`          | 📋     | Plugin scaffolding only; detection rules stubbed. Owned by another agent. |
| High Availability                        | [ha.md](./ha.md)                                  | n/a                          | 📋     | Design stub; owned by another agent. |
| Distributed Tracing                      | [tracing.md](./tracing.md)                        | `pcloud-observability`       | 📋     | Design stub; owned by another agent. |

## Landed implementors at a glance

| Trait              | Location                                      | Concrete implementor              | Default               |
|--------------------|-----------------------------------------------|-----------------------------------|-----------------------|
| `IdpBroker`        | `crates/pcloud-idp/src/lib.rs:258`            | `OidcAuthorizationCodeBroker`     | `UnimplementedBroker` |
| `PolicyEngine`     | `crates/pcloud-policy/src/lib.rs:196`         | `RegoPolicyEngine`                | `NullPolicyEngine`    |
| `FleetAgent`       | `crates/pcloud-fleet/src/lib.rs:238`          | `MtlsFleetAgent`                  | `NullFleetAgent`      |
| `KmsProvider`      | `crates/pcloud-kms/src/lib.rs:158`            | `AwsKms` / `HashicorpVault`       | `NullKms`             |

Every trait has a `Null*` implementor that is the default when the
corresponding `[section]` is absent from `pcloud-rs.toml`. This
preserves the single-user "works out of the box" behaviour while
letting operators opt into enterprise controls explicitly.

## Cross-cutting security invariants

These rules apply to every enterprise crate and are enforced in review:

- **Secrets in `SecretString` / `SecretBytes`** (`crates/pcloud-secret/`).
  No long-lived raw `String` / `Vec<u8>` holding credential material,
  tokens, keys, or PINs.
- **Owner-only file modes.** Identity files (`fleet.key`), vault files,
  policy bundles: `0600`, parent dir `0700`. Mode checks happen at
  load.
- **No credentials in the config file.** Config is for addresses,
  pointers, and policy knobs. Credentials come from the platform
  credential chain (AWS default provider, `VAULT_TOKEN` env, OS
  keyring).
- **TLS only.** No downgrade flags. `rustls` with explicit trust roots
  where the operator supplies a bundle.
- **Default-deny on error.** Policy engine denies on parse/eval error.
  KMS refuses to auto-fallback from a failed provider to a weaker one.

## FIPS posture (audit-06 LOW deployment / pcloud-rs-ncx.87-b)

pcloud-rs is **not** FIPS 140-2 / 140-3 validated today. Honest
statement for operators in FIPS-regulated environments:

- **Crypto primitives.** The Enhanced backend uses AES-256-GCM and
  Argon2id via `aes-gcm` and `argon2` from RustCrypto; the
  PclsyncCompat backend uses RSA-4096-OAEP and PBKDF2-HMAC-SHA512
  via the same suite. None of these crates ship a FIPS-validated
  module. There is no equivalent of OpenSSL's FIPS provider wired
  into the default build.
- **TLS.** `rustls` 0.23+ is the TLS stack. Mozilla's `rustls-fips`
  variant (built against BoringCrypto) is **not** currently wired
  into pcloud-rs. A reviewer wanting FIPS-aligned TLS must rebuild
  against `rustls-fips` and re-pin `webpki-roots` against the
  agency-approved CA bundle; no Cargo feature flag exists for this
  today.
- **Random.** We use `getrandom` which resolves to `/dev/urandom`
  on Linux, `BCryptGenRandom` on Windows, `arc4random_buf` on
  macOS. These are OS-provided CSPRNGs; they inherit whatever FIPS
  posture the underlying OS provides (RHEL FIPS-mode kernels
  satisfy SP 800-90A; stock Arch/Ubuntu do not claim FIPS mode).
- **Key storage.** Keys are held in `SecretBytes` (zeroize-on-drop)
  in-process memory. There is no PKCS#11 / HSM integration in the
  default build. The KMS provider crates (`AwsKms`,
  `HashicorpVault`) can delegate DEK wrapping to a HSM-backed KMS,
  but the pcloud-rs process still sees plaintext DEKs when it
  encrypts; that is not a FIPS boundary.

Operators who require a FIPS boundary should run pcloud-rs inside a
FIPS-mode kernel, delegate all sensitive key operations to a
HSM-backed KMS provider, and treat pcloud-rs itself as "operates in
a FIPS-enabled environment" rather than "is a FIPS module". A true
FIPS-module rebuild is out of scope for this repository and is not
on the roadmap.

## Honest posture (re-stated)

Honest caveats on the landed surface:

- **OIDC broker:** the IdP half is real. The pCloud-side
  trusted-issuer token exchange is stubbed pending pCloud API support;
  there is no "log in to pCloud via Okta today" claim.
- **Policy engine:** the engine, default-deny gate, file-permission
  guard, hot-reload, and four example policies are all in code.
  Operator production rollouts still require their own rule bundles.
- **Fleet agent:** tested against an in-process stub server. No
  reference fleet server ships in this repository; live fleet
  interop is not claimed.
- **KMS:** provider crates compile and pass offline tests, but the
  CryptoShell encrypt path still defaults to `NullKms`; wrapping
  real DEKs through `AwsKms`/`HashicorpVault` in production is
  tracked work, not shipped work.

Design stubs follow the same rigor as the landed docs — problem
statement, architecture, trust/security model, operator config, CLI
surface, failure modes — but they are roadmap artefacts. A stub is not
a promise that the feature will ship; it is a record of the shape the
feature would take if it did.

## Cross-references

- Security invariants: `docs/book/src/security/model.md`,
  `docs/book/src/security/secrets.md`
- CLI surface: `docs/book/src/cli/`
- Runbooks: `docs/runbooks/`
- Parity truth source: `STATUS.md`,
  `C_FEATURE_PARITY_MATRIX.csv`
- Handoff dossier: `CLAUDE.md` (repo root)

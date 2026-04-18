> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# Enterprise KMS / HSM Integration

Status: **LANDED (AwsKms / HashicorpVault / Pkcs11Hsm)** /
**LANDED (CryptoShell DEK routing — sector path wired)** /
**PRE-ALPHA (no live HSM proof)**.

Code lives under `crates/pcloud-kms/`. All three providers —
`AwsKms`, `HashicorpVault`, and `Pkcs11Hsm` — are implemented behind
opt-in Cargo features (`aws`, `vault`, `pkcs11`). The `Pkcs11Hsm`
provider binds to a vendor-supplied PKCS#11 module at runtime via the
`cryptoki` crate (Apache-2.0 / MIT) and performs AES-GCM wrap/unwrap
inside the HSM using `C_Encrypt` / `C_Decrypt`. The `KmsProvider`
trait and the `unwrap_cached` TTL cache are wired through the
runtime's KMS selector.

**DEK routing through `CryptoShell` is now wired.** `CryptoShell`
carries an injected `Box<dyn KmsProvider>` field (default:
`NullKms`), constructor `with_kms_provider(…)` and in-place setter
`set_kms_provider(…)` let the daemon replace the default with a
concrete provider built from the profile's `[crypto.kms]` section.
`CryptoShell::kms_wrap_dek` and `kms_unwrap_dek` route through the
injected provider; `kms_unwrap_dek` uses the shared
`unwrap_cached` TTL (`DEFAULT_CACHE_TTL` = 300 s) so repeated sector
opens do not round-trip to the KMS on every call. The field is
`#[serde(skip)]` — a deserialised `CryptoShell` always comes back with
`NullKms` and must be re-injected by the runtime (prevents a stale
wrapped-DEK from a previous provider being replayed against a new
deployment).

Honest remaining gap: the routing and provider surfaces are
**integration-tested** (`pcloud-crypto/tests/kms_routing.rs` — 8
tests covering NullKms raw regression, mock-provider KMS round-trip,
cache TTL proof, enable/seal/open/stop/restart cycle, and
reset-reverts-to-raw; plus the AWS / Vault opt-in integration tests
that require live credentials). There is still **no end-to-end live
HSM proof**, the `Pkcs11Hsm` bad-module-path test is the only pkcs11
regression that runs in CI, and the pre-alpha honesty rule says we do
**not** yet claim "production HSM support".

## Feature flags

All three providers are **off by default** and gated at compile time:

- `aws` — enables `AwsKms` (pulls in `aws-sdk-kms`, `aws-config`).
- `vault` — enables `HashicorpVault` (pulls in `reqwest` with
  `rustls-tls`, blocking).
- `pkcs11` — enables the real `Pkcs11Hsm` provider (pulls in
  `cryptoki`). When the feature is **off** the crate still exports a
  `Pkcs11Hsm` type whose constructor returns
  `KmsError::NotImplemented("pkcs11 (rebuild with --features pkcs11)")`
  — misconfigured deployments fail loudly instead of silently
  downgrading to `NullKms`.

The `pcloud-config` crate exposes a parallel set of feature names so
the `[crypto.kms]` factory can build the right provider:

- `kms-factory` — compile the `CryptoKmsConfig::build_provider` helper.
- `aws-kms` / `vault-kms` / `pkcs11-kms` — pass through to the
  matching feature on `pcloud-kms`.

A release build with no KMS feature compiles `NullKms` only plus the
`Pkcs11Hsm` "feature disabled" stub; the crate stays slim for
non-enterprise builds.

## Declarative config

Profiles carry an optional `[crypto]` section:

```toml
[crypto.kms]
provider = "vault"
url = "https://vault.example.com:8200"
transit_key = "pcloud-rs-kek"
token_env = "PCLOUD_VAULT_TOKEN"
```

Secrets (Vault token, PKCS#11 PIN) are **never** stored in the config
file. The config names an environment variable (`token_env`,
`pin_env`) and the factory reads the secret into
`pcloud_secret::secret_string::SecretString` at provider-construction
time.

## 1. Problem statement

The current `pcloud-crypto` crate derives the crypto-folder master
wrapping key entirely on the client, using Argon2id with the user's
crypto password as the only input entropy. That model has three
enterprise-unfriendly consequences:

1. **The password *is* the key.** Anyone who learns the crypto password
   (phishing, reused password, shoulder-surf, malware with a key logger)
   immediately has the wrapping key for every file in every crypto
   folder that password has ever protected. No server-side revocation
   can cut that access off, because the server never had the key.
2. **No revocation primitive.** To revoke a departed engineer's access
   to a crypto folder we would have to re-encrypt every file in the
   folder under a new key and push the ciphertext back to pCloud. For
   any non-trivial dataset this is operationally infeasible.
3. **No central audit.** Every unwrap happens on the endpoint. There is
   no authoritative log of "who decrypted folder X at 14:22 UTC" — only
   local syslog, which the user themselves can delete.

Enterprise operators want the wrapping key held by a KMS or PKCS#11 HSM
so that (a) the key never leaves the security boundary, (b) access is
revocable by flipping an IAM rule, and (c) every decrypt emits a
tamper-evident audit record owned by the KMS, not the endpoint.

## 2. Architecture — envelope encryption

The integration follows the standard **envelope / wrapping** pattern and
changes *only* the handling of the master key:

```
            ┌────────── pcloudd ──────────┐
 file  ───► │ per-sector AES-256-GCM      │ ───► pCloud object store
            │   with random DEK           │
            │                              │
            │ DEK ──► KMS.Encrypt() ──► wrapped-DEK (stored in metadata)
            │ DEK ◄── KMS.Decrypt() ◄── wrapped-DEK (read on unlock)
            └──────────────────────────────┘
                         ▲
                         │ KMS wrapping key never leaves the KMS
                         │
                 ┌───────┴────────┐
                 │ AWS KMS /      │
                 │ Vault Transit /│
                 │ PKCS#11 HSM    │
                 └────────────────┘
```

The per-file sector encryption, its AEAD choice (AES-256-GCM), and the
filename encoding stay exactly as they are today in `pcloud-crypto`.
The only behaviour that changes is how the per-folder DEK is
wrapped and unwrapped. This preserves backward compatibility with
ciphertext already on pCloud: a folder created in the local-Argon2 mode
can still be opened locally, and a folder created in KMS mode cannot be
opened by a client that has no KMS access — which is exactly the
property we want.

The trait surface lives in `pcloud-kms`:

```rust
pub trait KmsProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn encrypt_dek(&self, key_id: &KeyId, dek: &PlaintextDek,
                   context: Option<&str>) -> Result<WrappedDek, KmsError>;
    fn decrypt_dek(&self, key_id: &KeyId, wrapped: &WrappedDek,
                   context: Option<&str>) -> Result<PlaintextDek, KmsError>;
    fn health_check(&self) -> Result<(), KmsError>;
}
```

The crate ships four impls:

- `AwsKms` — **landed** behind the `aws` feature. Uses `aws-sdk-kms`
  (`Encrypt` / `Decrypt`) with `EncryptionContext`. The SDK is async;
  the trait is sync, so the provider bridges via
  `tokio::runtime::Handle::try_current()` and falls back to a
  lazily-built single-thread runtime for non-Tokio callers.
  Credentials come exclusively from the default provider chain (IMDSv2,
  env, shared credentials, SSO). The config file **never** carries AWS
  credentials; attempting to do so is a load error.
- `HashicorpVault` — **landed** behind the `vault` feature. Blocking
  `reqwest` client with `rustls-tls` against
  `/v1/transit/encrypt/<key>` and `/v1/transit/decrypt/<key>`. Vault
  token is read from `VAULT_TOKEN` (or `VAULT_TOKEN_FILE`) and sent as
  `X-Vault-Token`; it is never logged, never persisted in the config
  file, and wrapped in `SecretString`.
- `Pkcs11Hsm` — **stub**. Trait impl compiles under the `pkcs11`
  feature but every call returns `KmsError::NotImplemented`. The
  documented intent (`C_WrapKey` / `C_UnwrapKey`,
  `CKM_AES_KEY_WRAP_KWP`) and the PIN-from-keyring rule remain in
  place; no PKCS#11 module linkage is performed yet.
- `NullKms` — explicit "no KMS configured" mode. Returns
  `NotImplemented` for every call; it is **not** a fallback for a
  broken KMS, it is the selector for the legacy local-Argon2 path, and
  it is the default when no feature is enabled.

### `unwrap_cached` TTL cache

Both live providers are fronted by an `unwrap_cached(key_id, wrapped,
context)` helper that keys on `(provider name, key_id, wrapped blob
hash, context)` and returns an in-memory `SecretBytes` DEK. Entries
expire after `cache_ttl_seconds` (default 3600s, operator-configurable).
`PolicyDenied` and `Malformed` invalidate matching entries immediately.
Plaintext DEKs never hit disk; only the wrapped blob may be cached on
disk, `0600`, under `state_dir`.

## 3. IAM — per-device least privilege

Every device that mounts crypto folders gets its own identity. The
device's KMS policy allows exactly two operations:

- `Encrypt` against the configured wrapping key,
- `Decrypt` against the configured wrapping key.

Explicitly denied:

- `GenerateDataKey` / `GenerateDataKeyWithoutPlaintext` — we generate
  the DEK locally with `OsRng`, we do not ask the KMS for one, so this
  permission is never needed and enabling it widens the blast radius.
- `CreateKey`, `ScheduleKeyDeletion`, `PutKeyPolicy` — never needed
  from a client.
- `Decrypt` across other tenants' keys — scoped by
  `kms:EncryptionContext:device_id` so device A cannot unwrap blobs
  created for device B even if they share a CMK.

Revoking a device is one IAM flip: detach its policy. The next unwrap
call fails with `KmsError::PolicyDenied`, the daemon moves the folder
to read-only (§6), and the user sees a clear "access revoked" status.
No client data re-encryption is required.

## 4. Offline mode

A KMS round-trip per unlock is acceptable at login time, but round-
tripping for every file read is not — and some fleets need to work on
planes with no network at all. The design caches **wrapped** and
**plaintext** DEKs for a configurable TTL:

- **Wrapped cache**: `~/.local/state/pcloud-rs/kms-cache/<folder>.wrap`,
  mode `0600`. Just the wrapped blob. Always safe to cache — it is
  ciphertext. TTL defaults to 24h, configurable via
  `[crypto.kms] cache_ttl_hours`.
- **Plaintext cache**: in-memory only, inside the already-existing
  `SecretBytes` wrapper, evicted on screen lock / daemon exit / TTL.
  Never written to disk.

When the KMS is unreachable and the cache TTL has not expired, unlocks
succeed against the cache. When the TTL has expired, the folder goes
read-only (§6). Cache entries are invalidated whenever the KMS returns
`PolicyDenied`, so a revocation propagates as soon as the device
regains connectivity.

## 5. Key rotation

Rotation is **server-side only** and requires no client data
re-encryption, because of the envelope pattern:

1. Operator rotates the wrapping key inside the KMS (AWS KMS
   automatic annual rotation / Vault Transit `rotate` / HSM new key
   version).
2. New `Encrypt` calls produce wrapped blobs tagged with the new key
   version.
3. Old wrapped blobs still unwrap against the old key version, which
   the KMS keeps for decrypt-only.
4. Optional background re-wrap job walks the crypto folder metadata
   and reissues wrapped-DEKs under the new version; this is purely
   housekeeping and can run at the operator's convenience.

No sector data is ever rewritten during rotation. Compare this to the
legacy model, where "rotation" means re-encrypting every file.

## 6. Failure handling

Every unwrap goes through a small state machine:

- **KMS healthy** → normal operation.
- **KMS unreachable, cache valid** → serve from cache, log a warning,
  emit a `kms.degraded` metric.
- **KMS unreachable, cache expired** → the affected folder drops to
  **read-only**. Existing open file handles keep their in-memory DEKs
  but no new unwraps succeed. The daemon escalates via the existing
  `pcloud-observability` channel (structured log + optional
  PagerDuty/webhook), and the CLI surfaces a clear error.
- **`PolicyDenied`** → treat as permanent revocation: invalidate the
  cache, move folder to read-only, emit a `kms.revoked` audit event.
  Never retry silently.
- **`Malformed`** → fatal; do not retry. This almost certainly means
  tampering or an MITM in front of the KMS endpoint.

Read-only mode is strictly safer than "fall back to Argon2" — the
latter would let a KMS outage silently downgrade security, which is
exactly the failure mode enterprise buyers want to eliminate.

## 7. Operator configuration

New section in `pcloud-rs.toml`:

```toml
[crypto.kms]
provider           = "aws"             # "null" | "aws" | "vault" | "pkcs11"
key_arn            = "arn:aws:kms:eu-west-1:123456789012:key/abcd-..."  # aws only
vault_addr         = "https://vault.example.internal:8200"              # vault only
vault_path         = "transit/keys/pcloud-rs-wrap"                       # vault only
pkcs11_slot        = 0                                                  # pkcs11 only
pkcs11_label       = "pcloud-rs-wrap"                                    # pkcs11 only
cache_ttl_seconds  = 3600
```

`provider = "null"` (or an absent `[crypto.kms]` section) is the
default and maps to the legacy local-Argon2 path. Exactly one of
`key_arn` / (`vault_addr` + `vault_path`) / (`pkcs11_slot` +
`pkcs11_label`) must be set when `provider` is non-null; the daemon
refuses to start otherwise. The config file **must not** carry AWS
access keys or Vault tokens — those come from IMDS / env / shared
credentials (AWS) and `VAULT_TOKEN` (Vault). Any credential-shaped key
in the config file is a load error.

## 8. Security rules

These are non-negotiable and enforced by review:

- **KMS credentials never live in the config file.** AWS → default
  provider chain (IMDSv2, env, SSO). Vault → `VAULT_TOKEN` env or
  AppRole. PKCS#11 → PIN from OS keyring or env. A config-file-based
  credential is a rejected PR.
- **Plaintext DEKs never hit disk.** Wrapped in `SecretBytes`
  (zeroize-on-drop, redacted `Debug`), scoped to the narrowest
  possible function.
- **Wrapped DEKs may be cached on disk**, `0600`, in the user's state
  directory. They are ciphertext.
- **Never log key material.** The provider `name()` and the `KeyId`
  are safe to log; everything else is not.
- **Audit persistence failures are surfaced**, not swallowed, matching
  the workspace-wide rule in `CLAUDE.md`.
- **Transport**: TLS is mandatory for all three provider SDKs; no
  downgrade flags.

## 9. Interface / trait shape

Authoritative declarations:

- `KmsProvider` trait — `crates/pcloud-kms/src/lib.rs:158`
- `KmsError` — `crates/pcloud-kms/src/lib.rs:47`
- `KeyId` — `crates/pcloud-kms/src/lib.rs:96`
- `WrappedDek` (ciphertext, persistable) —
  `crates/pcloud-kms/src/lib.rs:113`
- `PlaintextDek` (zeroize-on-drop) —
  `crates/pcloud-kms/src/lib.rs:121`
- `DEFAULT_CACHE_TTL` (300 s) —
  `crates/pcloud-kms/src/lib.rs:151`
- `unwrap_cached(ttl)` (process-local, mutex-guarded) —
  `crates/pcloud-kms/src/lib.rs:198`
- `NullKms` (default, explicit refusal to be a "fallback") —
  `crates/pcloud-kms/src/lib.rs:278`
- `AwsKms` (feature `aws`) —
  `crates/pcloud-kms/src/lib.rs:334`
  - `::new(region)` at `:356`
  - Tokio handle bridge at `:393`
    (`tokio::runtime::Handle::try_current` fast path + fallback)
- `HashicorpVault` (feature `vault`, blocking rustls client) —
  `crates/pcloud-kms/src/lib.rs:512`
  - `::new(addr, path, token, …)` at `:542`
- `Pkcs11Hsm` (stub today) —
  `crates/pcloud-kms/src/lib.rs:677`
  - `::new(key_id)` at `:688`

```rust
// Simplified; see crates/pcloud-kms/src/lib.rs:158.
pub trait KmsProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn key_id(&self) -> &KeyId;
    fn generate_dek(&self) -> Result<(PlaintextDek, WrappedDek), KmsError>;
    fn decrypt_dek(&self, wrapped: &WrappedDek) -> Result<PlaintextDek, KmsError>;
    fn unwrap_cached(
        &self,
        wrapped: &WrappedDek,
        ttl: Duration,
    ) -> Result<PlaintextDek, KmsError> { /* default impl — cached */ }
}
```

## 10. Onboarding recipe

### Beginner — deploy in 5 steps (AWS KMS)

1. Create a CMK with `KeyUsage=ENCRYPT_DECRYPT` and `KeySpec=SYMMETRIC_DEFAULT`.
2. Grant the pCloud host role `kms:GenerateDataKey`, `kms:Decrypt`,
   `kms:DescribeKey` — **not** `kms:Encrypt`. We never encrypt
   plaintext with the CMK; we encrypt with the DEK.
3. Build the daemon with `--features aws`. Credentials come **only**
   from the default provider chain: IMDSv2, env, SSO. *Never* the
   config file.
4. Add `[crypto.kms]` with `provider = "aws"` and `key_arn`. Restart
   `pcloudcd`.
5. Encrypt a test file; inspect the CryptoShell header — it must carry
   a wrapped DEK, not a password-derived key.

### Expert — Terraform IAM

```hcl
data "aws_iam_policy_document" "pcloud-rs_kms" {
  statement {
    sid     = "PcloudccDek"
    actions = ["kms:GenerateDataKey", "kms:Decrypt", "kms:DescribeKey"]
    resources = [aws_kms_key.pcloud-rs.arn]
    # Explicitly omit kms:Encrypt, kms:ScheduleKeyDeletion, kms:*
  }
}

resource "aws_iam_role_policy" "pcloud-rs_kms" {
  role   = aws_iam_role.pcloud-rs_host.name
  policy = data.aws_iam_policy_document.pcloud-rs_kms.json
}
```

For Vault:

```hcl
resource "vault_mount" "transit_pcloud-rs" { path = "transit"; type = "transit" }
resource "vault_transit_secret_backend_key" "pcloud-rs" {
  backend = vault_mount.transit_pcloud-rs.path
  name    = "pcloud-rs-wrap"
  type    = "aes256-gcm96"
  deletion_allowed = false
}
```

`VAULT_TOKEN` is delivered via systemd `LoadCredential=` or the
`vault-agent` sidecar — **not** the pcloud-rs config file.

## 11. Verification

1. **NullKms unless configured** — start pcloudcd without
   `[crypto.kms]`; `pcloudc crypto status` must print `provider=null`
   (see `crates/pcloud-kms/src/lib.rs:278`).
2. **Cache TTL** — two back-to-back decrypts of the same DEK must
   produce exactly one CloudTrail `kms:Decrypt` event within the
   `cache_ttl_seconds` window. Cross-check
   `crates/pcloud-kms/src/lib.rs:198`.
3. **Credential prohibition** — try setting `aws_access_key_id` in
   `[crypto.kms]`; the config loader must reject it at parse time.
4. **Feature gating** — `cargo check -p pcloud-kms` (default features)
   must not pull in `aws-sdk-kms` or `vault-client` transitively.
5. **Tokio bridge** — under a non-tokio caller (sync CLI), a single
   `generate_dek()` must succeed; confirmation via `cargo test -p
   pcloud-kms --features aws aws_generate_dek_from_sync`.

## 12. Failure modes + remediation

| Symptom / `KmsError`              | Root cause                                                  | Remediation |
|-----------------------------------|-------------------------------------------------------------|-------------|
| `ProviderUnavailable`             | AWS throttling, Vault sealed, PKCS#11 token absent          | Consult offline cache; alert ops. Do **not** auto-fallback to a weaker provider. |
| `KeyNotFound`                     | CMK revoked or Vault key deleted                            | Escalate immediately; any cached DEKs continue to work for `cache_ttl_seconds` only. |
| `IamDenied`                       | Missing `kms:Decrypt` on the role                           | Fix the IAM policy per §3 / §10. |
| Credentials in config             | Operator pasted an AWS key                                  | Config load error; scrub key from `git log`, rotate. |
| `Pkcs11Hsm` returns `Unimplemented` | Feature is a stub today                                   | Track `pcloud-kms` tracker; no production use. |

## 13. Honest limitations (pre-alpha)

- **CryptoShell DEK routing is wired and tested.** `NullKms` remains
  the compiled-in default (single-user deployments); when a real
  provider is injected via `set_kms_provider`, `seal_sector` /
  `open_sector` route per-file key derivation through the KMS-wrapped
  DEK via `derive_sector_file_key` → `unwrap_active_dek` →
  `KmsProvider::unwrap_cached`. `enable_kms_mode` generates a fresh
  32-byte DEK from the OS CSPRNG and wraps it through the injected
  provider. `stop()` evicts the cache so the plaintext DEK does not
  outlive the session. Config `[crypto].mode = "raw" | "kms"` gates
  the path; `mode = "kms"` requires a non-null `[crypto.kms]` block
  (validated at load time by `CryptoConfig::validate`). Integration
  tests in `pcloud-crypto/tests/kms_routing.rs` cover: NullKms raw
  regression, mock-provider KMS round-trip with cache TTL proof,
  enable/seal/open/stop/restart/re-open cycle, and reset-reverts-to-raw.
- **`Pkcs11Hsm` is a stub** — the struct exists
  (`crates/pcloud-kms/src/lib.rs:677`), method bodies return
  `KmsError::Unimplemented`.
- **No HSM signing use case is shipped.** KMS providers are used only
  for DEK wrap/unwrap. Do not route signing keys through this trait;
  that belongs in a future `SigningProvider` trait.
- **Credentials-in-config is a design prohibition**, not a detected
  runtime drift. Review gates enforce it.

## 14. Extension points

- **New provider** (GCP KMS, Azure Key Vault). Add a Cargo feature,
  implement `KmsProvider` (`crates/pcloud-kms/src/lib.rs:158`). Never
  read credentials from the config file — always from the platform's
  native credential chain (workload identity, metadata service,
  managed identity).
- **Custom cache policy.** Override `unwrap_cached` for a provider
  with short-TTL DEKs. The default impl lives at
  `crates/pcloud-kms/src/lib.rs:198` and is sufficient for most
  providers.
- **HSM signing.** Out of scope for `KmsProvider`. File a design note
  and propose `SigningProvider`; do not bolt signing onto this trait.

## 15. Cross-refs

- CLI: `docs/book/src/cli/crypto.md`
- Runbook — KMS outage: `docs/runbooks/kms-outage.md`
- Crypto module: `crates/pcloud-crypto/src/lib.rs`
- Data residency companion: `docs/enterprise/data-residency.md` (owned
  by another agent)
- Parity row: `C_FEATURE_PARITY_MATRIX.csv`
  (`crypto.kms.*` rows — `Rejected` on legacy C, net-new in Rust)

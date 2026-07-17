# `pcloud-kms`

**Maturity:** Experimental / bounded

**Version:** `0.1.0`

**Directory:** `crates/pcloud-kms`

**Manifest:** [`crates/pcloud-kms/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/Cargo.toml)

Enterprise KMS / HSM wrapping-key integration for pcloud-rs.

## Feature-family profile

**Why it exists.** Allow enterprise operators to wrap data-encryption keys under external KMS or HSM control.

**What it is good for.** Null/default operation plus optional AWS KMS, HashiCorp Vault Transit, and PKCS#11 provider paths.

**Why it is good at that job.** A narrow KmsProvider contract, provider-specific feature gates, no silent downgrade, and zeroized plaintext DEKs isolate external key custody.

## Targets

| Cargo target | Kinds | Source |
|---|---|---|
| `pcloud_kms` | lib | [`crates/pcloud-kms/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs) |
| `coverage_surface` | test | [`crates/pcloud-kms/tests/coverage_surface.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs) |

## Direct dependencies

`aws-config`, `aws-sdk-kms`, `base64`, `cryptoki`, `getrandom`, `pcloud-secret`, `reqwest`, `serde`, `serde_json`, `thiserror`, `tokio`, `zeroize`

## Cargo features

| Feature | Enables |
|---|---|
| `aws` | `dep:aws-config`, `dep:aws-sdk-kms`, `dep:tokio` |
| `default` | empty marker |
| `pkcs11` | `dep:cryptoki`, `dep:getrandom` |
| `serde` | `dep:serde` |
| `vault` | `dep:reqwest`, `dep:base64`, `dep:serde_json`, `dep:serde` |

## File inventory (3)

| File | Kind | Role |
|---|---|---|
| [`crates/pcloud-kms/Cargo.toml`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/Cargo.toml) | Cargo manifest | `aws` feature — AWS KMS provider. Off by default. |
| [`crates/pcloud-kms/src/lib.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs) | library root | pcloud-kms — Enterprise KMS / HSM key-wrapping integration |
| [`crates/pcloud-kms/tests/coverage_surface.rs`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs) | test | \[test\] |

## Rust declaration index (104 total; 20 visible)

| Item | Visibility | Kind | Source | Documentation hint |
|---|---|---|---|---|
| `KmsError` | `pub` | enum | [`crates/pcloud-kms/src/lib.rs:48`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L48) | Errors returned by any \[`KmsProvider`\] implementation. The variants are intentionally coarse. Providers map v… |
| `VAULT_CONNECT_TIMEOUT` | `private` | const | [`crates/pcloud-kms/src/lib.rs:87`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L87) | Read the source/rustdoc for the exact contract. |
| `VAULT_REQUEST_TIMEOUT` | `private` | const | [`crates/pcloud-kms/src/lib.rs:89`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L89) | Read the source/rustdoc for the exact contract. |
| `KeyId` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:102`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L102) | An opaque identifier for the wrapping key inside the KMS. The string form is provider-specific: - AWS KMS: `a… |
| `fmt` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:105`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L105) | Read the source/rustdoc for the exact contract. |
| `WrappedDek` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:119`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L119) | A wrapped (ciphertext) data-encryption-key as returned by the KMS. This blob is what gets stored in pCloud cr… |
| `PlaintextDek` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:127`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L127) | A plaintext data-encryption-key. Lives in memory only long enough to encrypt a sector. This wrapper zeroizes… |
| `expose` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L133) | Borrow the plaintext bytes. The returned slice MUST be treated as sensitive — never log it, never persist it. |
| `clone_secret` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:142`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L142) | Audit-visible duplication of the plaintext DEK. Used by \[`KmsProvider::unwrap_cached`\] to hand callers an own… |
| `fmt` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:148`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L148) | Read the source/rustdoc for the exact contract. |
| `DEFAULT_CACHE_TTL` | `pub` | const | [`crates/pcloud-kms/src/lib.rs:157`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L157) | Default TTL for cached unwrapped DEKs. |
| `KmsProvider` | `pub` | trait | [`crates/pcloud-kms/src/lib.rs:164`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L164) | The operations a KMS-backed wrapping provider must support. The trait is deliberately tiny: the client never… |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:166`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L166) | Human-readable provider name, for logs and telemetry. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:173`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L173) | Wrap a plaintext DEK using the provider's managed wrapping key. `context` is an optional associated-data stri… |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:183`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L183) | Unwrap a previously wrapped DEK. `context` MUST match the value passed to \[`Self::encrypt_dek`\]. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:191`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L191) | Lightweight liveness probe. |
| `unwrap_cached` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:204`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L204) | Cache-backed unwrap. Returns a cached plaintext DEK if the wrapped blob has been unwrapped within the TTL. Ot… |
| `CacheKey` | `private` | struct | [`crates/pcloud-kms/src/lib.rs:232`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L232) | Read the source/rustdoc for the exact contract. |
| `CacheEntry` | `private` | struct | [`crates/pcloud-kms/src/lib.rs:239`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L239) | Read the source/rustdoc for the exact contract. |
| `cache` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:244`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L244) | Read the source/rustdoc for the exact contract. |
| `CACHE` | `private` | static | [`crates/pcloud-kms/src/lib.rs:245`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L245) | Read the source/rustdoc for the exact contract. |
| `cache_lookup` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:249`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L249) | Read the source/rustdoc for the exact contract. |
| `cache_store` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:260`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L260) | Read the source/rustdoc for the exact contract. |
| `evict_cached_dek` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:276`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L276) | Evict a specific `(provider, key_id, wrapped, context)` entry from the process-local DEK cache. Called by \[`C… |
| `cache_insert_at` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:295`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L295) | Read the source/rustdoc for the exact contract. |
| `NullKms` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:312`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L312) | No-op provider used when KMS integration is disabled. `NullKms` is not a fallback for a failed KMS — it is th… |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:315`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L315) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:319`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L319) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:328`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L328) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:337`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L337) | Read the source/rustdoc for the exact contract. |
| `AwsKms` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:368`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L368) | AWS KMS provider. Builds an `aws-sdk-kms` client against the default credential provider chain (IMDSv2, env,… |
| `fmt` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:375`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L375) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:390`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L390) | Construct a new AWS KMS provider bound to the given region. The underlying SDK client is created lazily on fi… |
| `client` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:397`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L397) | Read the source/rustdoc for the exact contract. |
| `encryption_context` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:410`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L410) | Read the source/rustdoc for the exact contract. |
| `run_async` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:422`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L422) | Read the source/rustdoc for the exact contract. |
| `map_aws_err` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:451`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L451) | Read the source/rustdoc for the exact contract. |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:472`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L472) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:476`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L476) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:499`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L499) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:522`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L522) | Read the source/rustdoc for the exact contract. |
| `HashicorpVault` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:546`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L546) | HashiCorp Vault Transit provider. Talks to `/v1/transit/encrypt/&lt;key&gt;` and `/v1/transit/decrypt/&lt;key&gt;` on a V… |
| `fmt` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:555`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L555) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:576`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L576) | Construct a new Vault Transit provider. `vault_url` is the base URL (e.g. `https://vault.example.com:8200`),… |
| `post_json` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:597`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L597) | Read the source/rustdoc for the exact contract. |
| `validate_vault_url` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:634`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L634) | Read the source/rustdoc for the exact contract. |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:653`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L653) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:657`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L657) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:680`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L680) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:706`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L706) | Read the source/rustdoc for the exact contract. |
| `pkcs11_stub` | `private` | mod | [`crates/pcloud-kms/src/lib.rs:738`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L738) | Read the source/rustdoc for the exact contract. |
| `Pkcs11Hsm` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:744`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L744) | PKCS#11 HSM provider (disabled in this build — enable the `pkcs11` Cargo feature to compile real HSM support… |
| `new` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:756`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L756) | Attempt to construct a PKCS#11 HSM provider. # Errors Always returns \[`KmsError::NotImplemented`\] when the `p… |
| `new_from_module` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:771`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L771) | Attempt to construct a PKCS#11 HSM provider from a vendor module path (e.g. `/usr/lib/softhsm/libsofthsm2.so`… |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:785`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L785) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:788`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L788) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:798`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L798) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:808`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L808) | Read the source/rustdoc for the exact contract. |
| `pkcs11_real` | `private` | mod | [`crates/pcloud-kms/src/lib.rs:820`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L820) | Read the source/rustdoc for the exact contract. |
| `Pkcs11Hsm` | `pub` | struct | [`crates/pcloud-kms/src/lib.rs:855`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L855) | PKCS#11 HSM provider. Wraps a \[`cryptoki::context::Pkcs11`\] handle against a vendor `.so` / `.dylib` PKCS#11… |
| `fmt` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:866`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L866) | Read the source/rustdoc for the exact contract. |
| `map_err` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:875`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L875) | Read the source/rustdoc for the exact contract. |
| `new` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:898`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L898) | Back-compat constructor used when the caller has no module path or PIN and only wants the "feature off" error… |
| `new_from_module` | `pub` | fn | [`crates/pcloud-kms/src/lib.rs:919`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L919) | Construct a new PKCS#11 HSM provider. - `module_path`: path to the vendor PKCS#11 shared library (e.g. `/usr/… |
| `find_key` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:953`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L953) | Read the source/rustdoc for the exact contract. |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:964`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L964) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:968`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L968) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1014`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1014) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1054`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1054) | Read the source/rustdoc for the exact contract. |
| `tests` | `private` | mod | [`crates/pcloud-kms/src/lib.rs:1081`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1081) | Read the source/rustdoc for the exact contract. |
| `null_provider_reports_name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1085`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1085) | Read the source/rustdoc for the exact contract. |
| `null_provider_refuses_ops` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1090`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1090) | Read the source/rustdoc for the exact contract. |
| `null_provider_health_is_ok` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1101`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1101) | Read the source/rustdoc for the exact contract. |
| `plaintext_dek_debug_is_redacted` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1106`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1106) | Read the source/rustdoc for the exact contract. |
| `pkcs11_constructor_fails_loudly` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1114`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1114) | Read the source/rustdoc for the exact contract. |
| `MockProvider` | `private` | struct | [`crates/pcloud-kms/src/lib.rs:1129`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1129) | Read the source/rustdoc for the exact contract. |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1133`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1133) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1136`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1136) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1148`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1148) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1159`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1159) | Read the source/rustdoc for the exact contract. |
| `trait_object_dyn_dispatch_roundtrips` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1165`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1165) | Read the source/rustdoc for the exact contract. |
| `pkcs11_bad_module_path_is_unreachable_or_other` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1182`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1182) | Read the source/rustdoc for the exact contract. |
| `CountingProvider` | `private` | struct | [`crates/pcloud-kms/src/lib.rs:1205`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1205) | Test provider that counts decrypt_dek invocations. |
| `new` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1210`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1210) | Read the source/rustdoc for the exact contract. |
| `call_count` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1216`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1216) | Read the source/rustdoc for the exact contract. |
| `name` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1221`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1221) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1224`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1224) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1232`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1232) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1241`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1241) | Read the source/rustdoc for the exact contract. |
| `cache_returns_plaintext_within_ttl` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1247`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1247) | Read the source/rustdoc for the exact contract. |
| `cache_expires_after_ttl` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1268`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1268) | Read the source/rustdoc for the exact contract. |
| `cache_distinguishes_context` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1296`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1296) | Read the source/rustdoc for the exact contract. |
| `aws_wrap_unwrap_roundtrip` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1318`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1318) | Read the source/rustdoc for the exact contract. |
| `vault_constructor_rejects_http_url` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1339`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1339) | Read the source/rustdoc for the exact contract. |
| `vault_constructor_rejects_url_credentials` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1354`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1354) | Read the source/rustdoc for the exact contract. |
| `vault_constructor_accepts_https_url` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1369`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1369) | Read the source/rustdoc for the exact contract. |
| `vault_wrap_unwrap_roundtrip` | `private` | fn | [`crates/pcloud-kms/src/lib.rs:1385`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/src/lib.rs#L1385) | Read the source/rustdoc for the exact contract. |
| `EchoKms` | `private` | struct | [`crates/pcloud-kms/tests/coverage_surface.rs:8`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L8) | Read the source/rustdoc for the exact contract. |
| `name` | `private` | fn | [`crates/pcloud-kms/tests/coverage_surface.rs:13`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L13) | Read the source/rustdoc for the exact contract. |
| `encrypt_dek` | `private` | fn | [`crates/pcloud-kms/tests/coverage_surface.rs:17`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L17) | Read the source/rustdoc for the exact contract. |
| `decrypt_dek` | `private` | fn | [`crates/pcloud-kms/tests/coverage_surface.rs:26`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L26) | Read the source/rustdoc for the exact contract. |
| `health_check` | `private` | fn | [`crates/pcloud-kms/tests/coverage_surface.rs:36`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L36) | Read the source/rustdoc for the exact contract. |
| `public_value_types_null_provider_and_cache_are_observable` | `private` | fn | [`crates/pcloud-kms/tests/coverage_surface.rs:42`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L42) | Read the source/rustdoc for the exact contract. |
| `disabled_pkcs11_constructors_fail_loudly` | `private` | fn | [`crates/pcloud-kms/tests/coverage_surface.rs:110`](https://github.com/ezechiel203/pcloud-rs/blob/main/crates/pcloud-kms/tests/coverage_surface.rs#L110) | Read the source/rustdoc for the exact contract. |

## Usage guidance

Treat this package as experimental, optional, enterprise-bounded, or unshipped until its feature and release evidence says otherwise.

# Cryptography, secrets, and key custody

pcloud-rs has several distinct cryptographic responsibilities. They must not
be collapsed into “encryption”:

```text
TLS                     protects traffic to pCloud / enterprise services
local IPC redaction     protects secrets crossing the owner-only daemon edge
auth-token vault        protects an optionally persisted login token
pcloud-secret           protects secret values inside process memory/APIs
Crypto folder           encrypts file content and names before pCloud storage
KMS/HSM mode            changes custody of the Crypto data-encryption key
audit-chain HMAC        detects local audit-log tampering
snapshot GPG            protects portable backup artifacts
plugin signatures       authenticate extension manifests/modules
```

No one mechanism substitutes for another: TLS does not hide plaintext from
the cloud service, a Crypto folder does not authenticate a local IPC peer, and
KMS wrapping does not make the process itself a validated cryptographic
module.

## Crypto backend decision

| Property | `PclsyncCompat` (default) | `Enhanced` (explicit opt-in) |
|---|---|---|
| Purpose | Interoperate with official pCloud Crypto clients and existing compatible ciphertext | Provide a Rust-native modern AEAD/KDF profile when pcloud-rs is the only reader |
| Official desktop/web/mobile interoperability | Intended and implemented through pclsync-v2 primitives/KATs | **No**; format is intentionally incompatible |
| Password KDF/profile | PBKDF2-HMAC-SHA512, 64-byte salt, 20,000 iterations, RSA-4096 profile material | Argon2id defaults used by the crate (`m=19456 KiB`, `t=2`, `p=1`), 16-byte salt, 32-byte master |
| Content | 4096-byte-compatible sector codec using pclsync AES modes, HMAC/tags, and auth tree | 4096-byte sectors sealed with AES-256-GCM, random 96-bit nonce, sector index as AAD |
| Filenames | Reversible pclsync-compatible encoding/base32 envelope | Deterministic NFC-normalized HMAC-SHA256 hex encoding; useful for equality lookup but not reversible by itself |
| Key/profile wrapping | RSA-4096 OAEP and pclsync private/public profile blobs | Domain-separated HMAC-derived file/folder keys |
| Best choice | Existing Crypto accounts, sharing with official clients, migration, or uncertainty | Closed pcloud-rs-only deployment that accepts permanent non-interoperability |
| Maturity caveat | Primitive and profile code plus KATs exist; every release still needs real cross-client qualification | Implemented internal format; not a pCloud-compatible format and must never be marketed as one |

The backend is persisted as profile truth. `CryptoShell::effective_backend`
does not guess from current build flags, and operations reject incompatible
context rather than silently falling back. The CLI requires explicit
acknowledgement before selecting Enhanced.

## Crypto lifecycle

| Feature | What and why it exists | Good for, and why | Entrypoint / maturity |
|---|---|---|---|
| `CryptoShell` | Owns backend identity, keys, content/metadata policy, encrypted-folder registry, KMS provider, DEK mode, lockout, and nonce counters. It exists as the single crypto state boundary. | Daemon integration and controlled internal embedding. One state owner prevents callers from encrypting while locked or mixing profiles. | `pcloud-crypto::CryptoShell`; internal implemented |
| Setup | Creates profile/KDF material, optional hint, and backend-specific keys. It exists to distinguish first-time key generation from later unlock. | Enabling Crypto on a dedicated account/profile. Backend choice and unsafe policy are validated before key work. | `setup[_with_backend]`; implemented; server-side setup requires live qualification |
| Start/unlock | Derives/unwraps active material and transitions `Locked → Unlocking → Unlocked`. It exists so plaintext operations require an explicit authenticated session. | Each daemon session and post-expiry reauthentication. Wrong-password comparison is constant-time and active keys remain secret-wrapped. | `start[_with_backend]`, `unlock`; implemented |
| Stop/lock | Drops active master/DEK state and returns to locked behavior. It exists to bound plaintext-key residence. | User lock, suspend, idle timeout, logout, and shutdown. `SecretBytes` zeroizes on drop and KMS cache entries can be evicted. | `stop`, `lock`; implemented |
| Reset | Removes setup/runtime state and returns to `NotSetup`. It exists for deliberate reinitialization, not ordinary lock. | Disposable profiles and recovery workflows. It clears key state, folders, mode, and counters coherently. | `reset`; implemented, destructive semantics require care |
| Status/hint | Exposes lifecycle/backend and a non-secret password hint. It exists so UX can explain state without reading secret material. | CLI status and support. Summaries are intentionally safe to log. | `summary`, `get_hint`, crypto status IPC; implemented |
| Password rotation | Verifies old/new values, rotates salt/fingerprint or re-encodes pclsync private material, records flags, and restores a usable state. It exists to change the protecting password without plaintext data export. | Routine rotation and temporary-password replacement. Constant-time identical-password checks and rollback-minded result types limit partial state. | `change_password*`, daemon crypto backend; implemented code; live happy path still needs out-of-band email OTP coordination |
| Persistent brute-force control | Counts failed unlocks across serialization, applies exponential backoff, caps attempts at 10, and caps wait at 30 minutes. It exists to make repeated local guessing expensive. | Stolen local profiles and unattended endpoints. Atomic counters survive restart so relaunch is not a free reset. | `consecutive_failures`, `last_fail_at`; internal implemented |
| In-memory key TTL | Evicts Enhanced active key material after a sliding default 300-second inactivity window (zero disables). It exists to bound exposure after an abandoned unlocked session. | Desktops and managed endpoints. Lazy checks at the key-use choke point need no background thread and zeroize before returning expiry. | `KeyManager::check_and_evict_if_stale`; implemented |
| Suspend/idle policy | Locks on suspend by default, forbids master-key persistence, and supports daemon-enforced idle auto-lock. It exists to express key-residency requirements as executable policy. | Laptops and strict deployments. Unsafe `persist_master_key=true` is refused before derivation. | `CryptoPolicy`; implemented policy, OS event wiring must be platform-qualified |
| Nonce budget | Persists a sector-seal count and refuses AES-GCM use near `u32::MAX` invocations per key, with a safety margin. It exists to enforce the NIST random-nonce collision bound. | Very large/long-lived Enhanced profiles. Rotation/reset creates a new key domain; process restart alone does not reset the budget. | `NONCE_BUDGET_SAFETY_MARGIN`, `sectors_sealed`; internal implemented |

## Enhanced backend primitives and helpers

| Feature/module | Rationale and behavior | Good for, and why | Security properties / limits |
|---|---|---|---|
| Argon2id master derivation (`keys`) | Derives 32 bytes from the Crypto password and a random per-profile 16-byte salt. It exists to raise offline-guess cost. | Rust-only profiles. Memory-hard Argon2id resists commodity parallel guessing better than a fast hash. | Password/master never serialized; OS swap/process compromise remain outside this guarantee. |
| Setup fingerprint | Stores `HMAC-SHA256(derived_key, "pcloud-crypto/fingerprint/v1")` instead of the key. It exists to reject wrong passwords before touching content. | Fast safe unlock validation. Constant-time comparison avoids prefix timing. | A stolen fingerprint still permits guesses at Argon2 cost; it is not a password verifier immune to offline attack. |
| Domain-separated per-file key | Derives an AES key with HMAC-SHA256 over a versioned file label/seed. It exists so one content key is not reused indiscriminately. | Sector encryption for many files under one master. Labels prevent overlap with fingerprints, filenames, and folder KEKs. | Isolation assumes HMAC PRF security and a trustworthy process while unlocked. |
| Per-folder KEK | Derives a 32-byte KEK from master and folder ID using a versioned HMAC/HKDF-expand-shaped label. It exists to avoid persisting raw folder KEKs. | Per-folder crypto policy and future independent wrapping. Different folder IDs produce distinct keys. | Current unlocked process can access all resident derived material; this is not process-level compartmentalization. |
| AES-256-GCM sector frames (`content`) | Seals up to 4096 plaintext bytes with a random 12-byte nonce, 16-byte tag, and big-endian sector index AAD. It exists for bounded random access with integrity. | Mounted/synced file chunks. Tampering, wrong key, wrong sector index, and oversize input fail authentication/validation. | Nonce uniqueness is probabilistic and enforced by a session budget; format is not official-client compatible. |
| Deterministic filename tag (`metadata`) | NFC-normalizes a leaf name and emits a fixed 64-character HMAC-SHA256 hex value. It exists to hide plaintext while preserving deterministic equality lookup. | Enhanced-only lookup and stable server names. Normalization reduces macOS/Linux Unicode-form drift. | It leaks equality under one key and is not reversible, so display requires a separate trusted mapping; `/` and empty names are rejected. |
| Folder policy registry | Records encrypted/plain overrides and runtime-only unlocked folders. It exists to model selective Crypto use rather than an account-wide boolean. | Keeping some folders plaintext and some gated. Persisted policy and ephemeral unlock membership are separate. | Internal T2.4 model; parent-chain completeness/integration must be verified before relying on inheritance as an access-control boundary. |
| Password scorer | Ports legacy quality scoring and passphrase-to-API-password derivation, with zeroized intermediate buffers and OWASP/legacy iteration constants. It exists for compatible UX and migration. | CLI guidance and legacy account flows. Keeping the scorer separate prevents its compatibility math from becoming the Crypto master KDF. | Scoring is advisory; it cannot force users to choose strong passwords. |
| Base64 helper | Centralizes encoding/decoding used by crypto wire helpers. It exists to remove divergent hand-written encoders. | Profile/share envelopes. One implementation reduces malformed-input differences. | Base64 is encoding, not encryption; decoded buffers must still use secret types where sensitive. |

## PclsyncCompat primitives and helpers

These modules are compiled by the default `pclsync-v2` Cargo feature. They
exist because interoperability requires byte-for-byte compatibility, even
where a new design would choose different primitives.

| Feature/module | What it does and rationale | Good for, and why | Important invariant / maturity |
|---|---|---|---|
| PBKDF2 KEK (`pclsync_kdf`) | Derives the compatible AES key/IV from password and a 64-byte salt using PBKDF2-HMAC-SHA512 at 20,000 iterations. | Unlocking existing official-client profile blobs. Exact constants are versioned and KAT-tested. | Compatibility choice, not a claim that 20,000 PBKDF2 iterations is the preferred new password KDF. |
| RSA-4096/OAEP (`pclsync_rsa`) | Generates/parses RSA keys and wraps/unwraps the pclsync `SymKeyVer1` using OAEP-SHA1/MGF1-SHA1. | Official-client key bundles and Crypto sharing. DER and fixed-size bundle codecs preserve layout. | SHA-1 is retained only where the compatibility format specifies it; not for new signatures. |
| AES modes (`pclsync_modes`) | Implements compatible AES-256-CTR private-key wrapping and AES-256-CBC-CS3 sector data transformation. | Exact C-client ciphertext processing. Dedicated functions isolate nonstandard compatibility modes. | Must be judged by KAT/official fixture equivalence, not roundtrip tests alone. |
| Sector codec (`pclsync_sector`) | Seals/opens 4096-byte sectors with compatible random/tweak/auth layout and 32-byte authentication tag. | Reading/writing official-format Crypto files. Sector/file context is explicit so keys cannot be used anonymously. | Byte-level KATs and malformed-input tests are the evidence boundary. |
| 128-ary authentication tree (`pclsync_auth_tree`) | Builds/verifies Merkle-like levels over sector tags and computes the master authenticator. | Integrity verification and random-access official Crypto files. Fanout and level bounds match the format. | Authentication tree state must stay consistent with sector publication; code presence alone is not live filesystem proof. |
| Reversible filenames (`pclsync_filename`) | Encodes/decodes names using compatible keys and base32 envelope, with length and malformed-text checks. | Directory listing across pCloud desktop/mobile/web and pcloud-rs. Reversibility preserves display names unlike Enhanced HMAC tags. | Exact Unicode/byte behavior requires cross-client fixtures on every relevant platform. |
| Profile codec (`pclsync_compat_profile`) | Builds/parses `priv_key_ver1`/`pub_key_ver1`, rewraps the private key, and caches live RSA/folder/file symmetric keys. | Setup, unlock, password change, and share acceptance. Runtime caches are separate from persisted opaque blobs. | RSA/private key material is secret; serialization only persists the compatible encrypted envelope. |
| RSA share invitation (`share_rsa`) | Parses recipient public keys and wraps a folder/file `SymKeyVer1` for a user or team target. | Interoperable Crypto folder sharing. Target type and payload encoding are explicit. | API reachability exists; bilateral real-account acceptance still needs credentialed live proof. |
| Temporary-password share (`share_temppass`) | Derives an AEAD/HMAC-protected temporary-password envelope and accepts it on the recipient side. | Migration and server-issued temporary Crypto password workflows. Detached authentication and secret wrappers strengthen local handling. | A temporary password must be rotated; this is a sharing compatibility path, not a general password transport. |
| Known-answer tests | Decrypt extracted pclsync fixtures and test exact modes/sectors/profile/filename behavior. They exist because “encrypt then decrypt ourselves” can preserve the same wrong algorithm. | Release compatibility evidence. Offline fixtures are reproducible; live KATs prove the current account/service path when explicitly enabled. | Extracted fixture provenance/password and cross-client producer version must be recorded. |

## Secret-value discipline (`pcloud-secret`)

| Feature | Why it exists | Good for, and why | Guarantee / limit |
|---|---|---|---|
| `SecretString` | Wrap UTF-8 passwords, tokens, PINs, and client secrets so ordinary Rust derives cannot leak them. | Auth, vault, OIDC, Vault, PKCS#11, and prompts. | Zeroize on drop, redacted Debug, no automatic Clone/serde, constant-time equality. It cannot stop a compromised process reading memory while exposed. |
| `SecretBytes` | Apply the same rules to keys, DEKs, MAC material, and binary tokens. | Crypto/KMS internals. | Heap buffer is zeroized on drop; explicit copies remain possible only through review-visible helpers. |
| `ExposeSecret` | Makes plaintext access an explicit grep-able operation. It exists so review can enumerate every declassification point. | Passing a secret to a protocol/crypto/OS API for the shortest possible scope. | The returned reference is plaintext; caller discipline still matters. |
| No serialization | Compile-fail tests prevent secret wrappers from gaining serde support. It exists to stop a containing struct from silently persisting credentials. | Profiles, IPC models, and logs. | Protocol boundaries must deliberately unwrap into redacted secret wire types where transmission is intended. |
| Constant-time comparison | Uses `subtle::ConstantTimeEq`. It exists to avoid early-exit prefix timing for tokens/MACs/password fingerprints. | Authentication and integrity comparisons. | Does not make surrounding parsing, allocation, or external services constant-time. |
| Log redaction helper | Replaces sensitive key/value fields with stable redacted tokens. It exists for defensive logging at subsystem boundaries. | Diagnostics and audit events. | Prevention still requires callers not to format an already-exposed plaintext. |

## Auth-token vaults

Only an authentication token may be persisted, and only after explicit
opt-in. Crypto passwords, account passwords, master keys, plaintext DEKs, and
private Crypto keys are not eligible for the auth-vault trait.

| Backend | Why it exists / best use | Why it is effective | Maturity |
|---|---|---|---|
| Auto selector | Chooses platform-native storage and surfaces fallback warnings. | Explicit backend requests never silently fall back; only `auto` may choose the portable file with a warning. | Implemented |
| Owner-only file | Gives every Unix-like/portable target a baseline. | Parent/file mode and ownership validation, atomic replacement, and secret wrapping make mispermission a hard error. | Implemented; filesystem/backup policy still matters |
| macOS Keychain | Delegates token protection and access control to the current user's Keychain. | Uses the platform security boundary rather than inventing local encryption keys. | Implemented source; native codesign/keychain qualification required |
| Windows DPAPI | Protects the token to the current Windows user and uses secure DACL-aware file replacement. | The ciphertext is bound to Windows user context and the storage file is owner-scoped. | Implemented source; must be tested in the actual interactive/service identity |
| Linux Secret Service | Stores through Freedesktop Secret Service when explicitly/automatically available. | Integrates with the desktop keyring and falls back only through surfaced `auto` policy. | Opt-in implemented; headless systems commonly use file fallback |

## Enterprise envelope encryption and key custody

`CryptoMode::Raw` derives/holds local key material. `CryptoMode::Kms` stores an
opaque wrapped DEK plus key ID and optional context; the injected provider is
never serialized and must be reconstructed after restart.

```text
random plaintext DEK (memory only)
      │
      ├── sector encryption inside pcloud-rs
      └── KmsProvider.encrypt_dek(key, context)
                        │
                        ▼
        wrapped DEK (safe to persist with metadata)

open: wrapped DEK → provider/cache → plaintext DEK → sector open → zeroize
```

| Feature/provider | Why it exists | Good for, and why | Maturity / caveat |
|---|---|---|---|
| `KmsProvider` | Normalizes wrap, unwrap, health, and cached unwrap across vendors. | Runtime injection and uniform failure handling. A tiny contract avoids leaking vendor SDK types into Crypto. | Internal implemented |
| `NullKms` | Represents “no external custody configured” explicitly. | Ordinary single-user Raw mode and safe defaults. It returns not-implemented for KMS calls; it is never a silent fallback after provider failure. | Implemented default |
| AWS KMS | Uses AWS Encrypt/Decrypt and encryption context through the standard credential chain. | IAM revocation, CloudTrail, and managed/HSM-backed wrapping keys. | Implemented behind `aws`; requires live account/IAM/region qualification |
| HashiCorp Vault Transit | Uses HTTPS Transit encrypt/decrypt with a secret-wrapped token. | Self-hosted central key policy and audit. | Implemented behind `vault`; requires live Vault/TLS/policy qualification |
| PKCS#11 HSM | Loads an operator vendor module, selects a slot/key label, logs in with a secret PIN, and performs AES-GCM in the token. | On-premises hardware custody. The feature-off type fails loudly rather than pretending support. | Real implementation behind `pkcs11`; no end-to-end hardware proof in the repository, therefore pre-alpha |
| Context binding | Passes folder/device/tenant context as provider AAD where supported. | Preventing a wrapped DEK replay into another context. | Caller and provider must use the identical context; provider capabilities vary |
| In-memory unwrap cache | Caches plaintext DEKs process-locally for a default 300 seconds, keyed by provider/key/blob/context, then zeroizes on eviction. | Avoiding one remote KMS request per sector. | Implemented; it increases the authorized in-memory window and is not a disk cache |
| KMS configuration factory | Constructs the selected provider from typed config while reading token/PIN from named secret environment sources. | Declarative enterprise deployment without credentials in config. | Feature-gated in `pcloud-config`; each provider build must be selected explicitly |
| KMS mode routing | Injects provider into `CryptoShell`, wraps/unwraps the DEK, persists only the wrapped blob, and reverts safely on reset. | Sector encryption under external custody. | Integration-tested with mocks; real provider/HSM release evidence remains external |

## Transport, audit, snapshot, and plugin cryptography

| Feature | Why it exists and what it does | Good for, and why | Entrypoint / caveat |
|---|---|---|---|
| rustls TLS | Authenticates/encrypts pCloud HTTPS and enterprise HTTP integrations with configured SNI/trust. | Protecting credentials and content in transit. Shared configuration prevents each API family choosing weaker defaults. | `pcloud-proto::tls`; certificates, roots, clocks, and proxy policy remain deployment concerns |
| Tamper-evident audit chain | Links persisted audit records with HMAC/hash material and verifies ranges. | Detecting local record deletion/rewrite and embedding tail proof in recovery artifacts. | store audit repository + daemon verifier; detection is not remote immutable storage |
| GPG snapshot encryption | Encrypts completed snapshot artifacts to an operator recipient. | Portable/off-site backups whose storage is not trusted. | snapshot backend invokes installed `gpg`; key ownership, expiry, and recovery tests are operator responsibilities |
| Ed25519 plugin signatures | Verifies signed plugin manifests/modules before registration. | Extension provenance. Capability checks remain necessary because a valid signature does not make code harmless. | `pcloud-plugin-api`; trust-root distribution is deployment policy |
| Secret-bearing IPC/proto wrappers | Redact sensitive request fields while allowing intentional encrypted/local transmission. | Password, token, link password, OIDC, and Crypto commands. | `pcloud-ipc::redacted`, `pcloud-proto::redacted`; local peer/TLS authentication must also succeed |

## FIPS posture

<span class="atlas-unqualified">Not FIPS validated</span>

- The default RustCrypto stack is implemented and reviewed but is not a
  FIPS-140 validated module.
- The `crypto-provider-aws-lc-fips` feature is an intentional compile-time
  refusal/forward seam. Enabling it does **not** produce a binary.
- Argon2id in Enhanced is not a FIPS-approved password KDF. PclsyncCompat also
  carries fixed compatibility primitives that cannot simply be relabeled.
- An AWS/Vault/HSM wrapping key can improve custody and audit, but pcloud-rs
  still sees plaintext DEKs to encrypt/decrypt sectors; that does not put the
  process inside the external module's validation boundary.
- A future validated build requires an actual validated provider integration,
  pinned build boundary, runtime enforcement, wire-format revalidation, and
  release-specific certification evidence described in `docs/fips.md`.

The generated [`pcloud-crypto`](../generated/crates/pcloud-crypto.md),
[`pcloud-kms`](../generated/crates/pcloud-kms.md), and
[`pcloud-secret`](../generated/crates/pcloud-secret.md) pages enumerate every
public and private Rust declaration and every crypto test, example, benchmark,
fuzz target, and build helper.


# Secrets Handling

This chapter documents how `pcloud-rs` represents, stores, and destroys secret material: passwords, auth tokens, crypto keys, and HMAC keys. It is the concrete implementation of the posture described in the [Security Model](./model.md).

The governing rule is simple: **a secret should exist in memory only as long as it is needed, never touch a log or an error message, and never be persisted in cleartext**. Every deviation from that rule is explicit and documented.

## Secret Wrapper Types

All long-lived secret material on the daemon side is held in one of two wrapper types defined in `crates/pcloud-secret/`:

### `SecretString`

A UTF-8 string wrapper used for passwords, TFA codes, recovery codes, and auth tokens.

- storage is a heap `String` that is overwritten with zero bytes on `Drop` via `zeroize`;
- `Debug` and `Display` are both redacted — they render as `SecretString(<redacted>)` — so accidental `{:?}` formatting cannot leak the plaintext;
- `Clone` is **not** derived; callers must use `SecretString::fork`, which re-allocates into a fresh zeroising buffer, so duplication is visible in review;
- `Serialize`/`Deserialize` are **not** derived; a compile-fail test pinned in `crates/pcloud-secret/tests/compile_fail/` prevents accidental JSON or YAML leakage;
- the inner value is accessed through `expose_secret()`, which returns `&str`. The method name is deliberate: every call site is grep-able and is reviewed as a security-sensitive surface.

### `SecretBytes`

A `Vec<u8>` wrapper used for symmetric keys, derived key material, HMAC keys, GCM nonces in transit, and any raw binary secret.

- the underlying `Vec<u8>` is zeroised on `Drop`, including trailing capacity so a grown-and-shrunk vector is not left with plaintext tails;
- `Debug` renders length only, never bytes;
- the type is `!Copy` and `Clone` is not derived; `fork` is explicit, matching `SecretString`;
- `copy_from_slice` and `xor_into` panic on length mismatch on the theory that a length mismatch in a cryptographic path is always a bug.

### Rules enforced at review time

- no raw `String` or `Vec<u8>` on any long-lived struct that transitively holds authentication or key material;
- no `println!` / `tracing::info!` / `error!` of a `SecretString` or its `expose_secret()` output;
- no `format!("{e}")` on an error type that embeds a secret — error types use `thiserror` with explicit redaction;
- constant-time comparison via `subtle::ConstantTimeEq` on every secret equality check (password verifies, MAC tags, token matches).

## Vault Backends

The daemon supports four vault backends for auth token persistence, selected at runtime based on platform and operator preference. All backends are write-through and all reads are explicitly requested — the daemon does not "warm-cache" a secret from a vault at startup unless an operation requires it.

### 1. File vault (default, cross-platform fallback)

Location:

- Unix: `$XDG_DATA_HOME/pcloud-rs/vault.json` (typically `~/.local/share/pcloud-rs/vault.json`);
- Windows: `%LOCALAPPDATA%\pcloud-rs\vault.json`.

Properties:

- Unix: parent directory enforced to mode `0700`, file enforced to mode `0600`, both owned by the daemon UID. The daemon **fails closed** if either check does not hold and does not attempt to auto-repair (ADR 0005);
- Windows: the vault directory is created with an ACL limited to the current user's SID, and the file inherits that ACL. This is **intentionally documented as a weaker guarantee** than Unix mode bits, because NTFS ACLs can be modified by an administrator or an attacker with `SeTakeOwnershipPrivilege`. Operators on Windows who need stronger protection should select the DPAPI backend;
- the on-disk format is versioned JSON with an HMAC-SHA256 over the serialised bytes, keyed by a per-install root key stored in the platform keystore where available;
- the vault never stores a password — only auth tokens and metadata (username hint, expiry, endpoint).

### 2. macOS Keychain

Implementation: `security-framework` crate, generic password items with a `com.pcloud-rs.auth` service identifier.

Properties:

- items are ACL-protected to the calling application bundle / signing identity;
- the Keychain item is created with `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`, so the token never roams to iCloud Keychain and is unavailable when the device is locked;
- deletion on `logout` or `vault clear` is synchronous and verified.

### 3. Windows DPAPI

Implementation: the `windows` crate, `CryptProtectData` / `CryptUnprotectData` with `CRYPTPROTECT_LOCAL_MACHINE = 0` (user scope).

Properties:

- the ciphertext blob is stored at the vault path, but the encryption key is derived from the user's logon credentials by the OS — an attacker who copies the blob to another machine cannot decrypt it;
- the optional `CRYPTPROTECT_AUDIT` flag is set so that access is logged in the Windows event log;
- DPAPI is the recommended backend on Windows for operators who need stronger-than-ACL protection.

### 4. Linux Secret Service (opt-in)

Implementation: `secret-service` crate over D-Bus.

Properties:

- stored as an item in the default collection (`login`), with attributes `service=pcloud-rs`, `account=<username_hint>`;
- **opt-in only** — the default on Linux is the file vault, because headless servers and CI hosts frequently lack a running `gnome-keyring` / `kwallet` daemon and we prefer a deterministic fallback;
- the daemon detects a missing Secret Service provider and emits an explicit error rather than silently falling back.

### Opt-in durable persistence

Regardless of backend, **durable auth token persistence is opt-in** via the environment variable `PCLOUD_DURABLE_AUTH_TOKENS=1` (or the equivalent config knob `auth.durable_tokens = true`). When the variable is unset (the default), the daemon keeps tokens in memory for the lifetime of the process and requires re-authentication on restart. This is a deliberate tightening over the legacy C client, which persisted credentials by default.

## Passwords Are Never Persisted

The legacy C client stored the user's account password in its local database to enable silent re-login. The Rust rewrite **does not carry this behaviour forward**. See [ADR 0007](../../../adr/0007-crypto-password-not-persisted.md) for the rationale. In practice:

- the password is accepted as a `SecretString` on the IPC `login` RPC;
- it is used immediately to obtain an auth token and is dropped before the RPC returns;
- the auth token is what gets persisted (when persistence is opted in);
- re-login after token expiry requires a fresh user interaction — there is no daemon path that reads a password from disk.

The same rule applies to the crypto password: it is used to derive the crypto master key via Argon2id, the derived key is cached in `SecretBytes` for the duration of the crypto session, and the password itself is dropped. Unlock-on-start is not supported — the operator must unlock crypto explicitly. Operators who need unattended unlock must script it through the IPC surface with a secret retrieved from an external secret store (HashiCorp Vault, AWS Secrets Manager, etc.).

## Cryptographic Primitives

The `pcloud-crypto` crate is the single owner of cryptographic code on the retained path.

### Key derivation — Argon2id

- parameters: `m = 64 MiB`, `t = 3`, `p = 1`, salt = 16 random bytes per user, output = 32 bytes;
- the salt is stored alongside the encrypted master key; the password is never stored;
- parameters are version-tagged so future increases can be applied without breaking existing vaults.

### Sector sealing — AES-256-GCM

- sector size: 4096 bytes of plaintext;
- nonce: 96 bits, constructed as `sector_index || random_prefix` so nonce reuse across files is not possible;
- associated data binds the file ID and sector index, preventing cut-and-paste attacks between sectors;
- authentication tag: 128 bits, verified before plaintext is returned.

### File-level integrity — HMAC-SHA256

- every encrypted file carries an HMAC-SHA256 tag over the ciphertext and metadata, keyed by a separate MAC subkey derived from the master key;
- verification uses `subtle::ConstantTimeEq` — no early exit on mismatch.

### Constant-time comparisons

Every equality check on secret material — password verifies, HMAC tags, GCM tags, token lookups — goes through `subtle::ConstantTimeEq`. There are no `==` comparisons on secret bytes anywhere in the crypto or auth path.

## Operator Checklist

- set `PCLOUD_DURABLE_AUTH_TOKENS=1` only on trusted single-user machines;
- on Windows, prefer the DPAPI backend over the file vault;
- never pass passwords on the command line; use `--password-stdin` or the interactive prompt;
- rotate the crypto password with `pcloudc crypto passwd` rather than re-encrypting manually;
- confirm `pcloudc doctor` reports "vault permissions OK" after any manual intervention.

See the [Threat Model](./threat-model.md) for how these controls map onto STRIDE categories and the residual risks that remain.

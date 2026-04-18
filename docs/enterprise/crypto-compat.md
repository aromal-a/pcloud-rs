> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# Crypto Cross-Client Compatibility

## Crypto Backend Selection (READ FIRST)

Two crypto backends are available in `pcloud-rs`. They are **wire-incompatible**
by design. Choose once, at `crypto setup` time — switching later requires
re-encrypting all content.

### Backend summary

| Property | `pclsync-compat` (default) | `enhanced` (opt-in) |
|---|---|---|
| Interoperable with pCloud desktop/web/mobile apps | **Yes** | **No** |
| Interoperable with pCloud iOS / Android | **Yes** | **No** |
| KDF | PBKDF2-HMAC-SHA512 (20 000 iters) | Argon2id (m=19456, t=2, p=1) |
| Sector cipher | Custom AEAD (HMAC-SHA512 tweak + CBC-CTS + AES-ECB-wrapped tag) | AES-256-GCM (16-byte tag, random 96-bit nonce) |
| Authentication tree | 128-ary Merkle HMAC-SHA512 | Per-sector AEAD tag (no separate tree) |
| Key wrapping | RSA-4096-OAEP + PBKDF2 KEK | HMAC-SHA256 KDFs (domain-separated) |
| Master key zeroized on Drop | Yes (`SecretBytes`) | Yes (`SecretBytes`) |

### Decision table

**Choose `pclsync-compat`** (default, no extra flag needed) when:

- Users also access the pCloud drive via the official desktop, web, or mobile apps.
- You need to decrypt existing encrypted content created by `pcloudcc` or the pCloud Drive app.
- You are migrating from the legacy C client and need uninterrupted access to existing Crypto folders.
- You are unsure — this is always the safer choice for interoperability.

**Choose `enhanced`** (opt-in, requires `--acknowledge-not-interop`) when:

- The Rust client is the **only** client that will ever access this account's encrypted content.
- You want stricter per-sector AEAD guarantees and a modern KDF (Argon2id).
- You are running an isolated enterprise deployment with no pCloud-app users.
- You have explicitly accepted that files encrypted with `enhanced` **cannot be opened
  by any official pCloud application**, including pCloud Drive, iOS, Android, and the web vault.

### CLI invocation

```bash
# Default (pclsync-compat — no extra flag):
pcloudc crypto setup

# Enhanced — explicit acknowledgement required:
pcloudc crypto setup --backend enhanced --acknowledge-not-interop
```

Scripts may pass `--backend {pclsync-compat|enhanced}`. The daemon logs
`crypto unlocked: backend=<NAME>` at every unlock. `pcloudc crypto status`
shows the active backend on its first output line.

### Warning: Enhanced files cannot be decrypted by pCloud apps

> **If you choose `enhanced`, files encrypted by this client are permanently
> inaccessible to the official pCloud desktop application, pCloud Drive,
> pCloud iOS, pCloud Android, and the pCloud web vault.** There is no migration
> path from `enhanced` back to `pclsync-compat` without full re-encryption.
> Do not choose `enhanced` if any user on the account uses official pCloud apps.

### Further reading

- Rollout plan and phased implementation: [`docs/CRYPTO-BACKEND-PLAN.md`](../CRYPTO-BACKEND-PLAN.md)
- pclsync crypto scheme reference: [`docs/crypto-reference-pclsync.md`](../crypto-reference-pclsync.md)
- Security threat model comparison: see the "Two-backend threat model" section below (in `docs/enterprise/security-model.md` if it exists)

---

## Warning: NOT Byte-Compatible With Legacy C Client

The Rust `pcloud-rs` crypto implementation (`crates/pcloud-crypto`) uses the
following primitives:

- **Cipher:** AES-256-GCM (sector-level AEAD)
- **Key derivation:** Argon2id (m=19456, t=2, p=1), 16-byte per-profile salt,
  32-byte output
- **Per-file key:** `HMAC-SHA256(master_key, "pcloud-crypto/file-key/v1" || file_seed)`
- **Filename encryption:** deterministic `HMAC-SHA256(master_key, "pcloud-crypto/filename/v1" || name)`
  NOTE: NFC normalization is NOT currently applied; filename bytes are hashed as-is (raw UTF-8).
  Cross-client compatibility with NFD-normalized filenames (common on macOS) is an open issue
  tracked under bd-1du. Do not assume cross-platform filename lookup will work for non-ASCII names.
- **Nonce:** 96-bit from OS CSPRNG (random per sector)
- **AAD:** sector index (4-byte big-endian) — note: an earlier version of this
  document incorrectly stated little-endian; the code (`content.rs`,
  `seal_sector`) uses `to_be_bytes()` and is authoritative

This format is **NOT byte-compatible** with the legacy C pCloud client
(`pcloudcom/pcloud-rs`, `pclsync/pcryptofolder.c`). The C client uses a
different crypto scheme (exact primitives TBD — see `bd-1du.10`).

## Practical Consequences

- Users who have encrypted folders created with the legacy C client **cannot
  access that content with the Rust client**.
- Users who encrypt new content with the Rust client **cannot read it with the
  C client**.
- There is currently **no migration path**. A migration tool is tracked under
  `bd-1du.10`.

## How to Know if This Affects You

If you have previously used the official pCloud desktop/CLI client on Linux
and enabled the Crypto folder feature, your existing encrypted data was
encrypted with the C client's scheme. Do not use the Rust client to access
those files until cross-client KAT compatibility is confirmed in
`crates/pcloud-crypto/tests/round_trip.rs` (renamed from `kat_compatibility.rs`).

## Migration Path

Migration path is TBD. Track under `bd-1du.10`.

Until `bd-1du.10` is resolved with a passing cross-client KAT:

- Do not claim cross-client file access.
- Do not describe the Rust crypto as a "drop-in replacement" for the C crypto.
- Surface this warning to end users before they enable the crypto feature on
  an account that has existing C-client encrypted content.

## Related Security Posture

The Rust crypto path is intentionally **stricter** than the C legacy path:

- Master key material is zeroized on drop (`SecretBytes`).
- No password is ever persisted to disk.
- Auth token persistence is opt-in and owner-mode-only.
- IPC sockets are owner-only.
- Brute-force lockout: after 5 consecutive wrong-password attempts the crypto
  layer refuses further unlock attempts until `reset()` is called.

These improvements are intentional divergences from the C client and are not
considered compatibility breaks.

> **Pre-alpha scaffold — not live / not production-verified.** This document
> describes design and unit-tested code that has not been validated against a
> real production deployment. Do not treat it as a shippable capability.
> See `CLAUDE.md` and `docs/enterprise/README.md` for the honesty rules.

# Crypto Cross-Client Compatibility

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

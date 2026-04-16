# pcloud-crypto

Client-side crypto folder primitives for pcloud-rs: AES-256-GCM sector
encryption, Argon2 key derivation, zeroized key material, and the share/
team-share temppass flow.

## What this crate does

- Implements the active Rust crypto-folder path: setup, start/stop, lock/unlock,
  and folder creation.
- Performs AES-256-GCM per-sector content encryption and deterministic
  metadata-filename encoding.
- Wraps all key material in `SecretBytes`, zeroizing on drop.

## Public API entry points

- `CryptoSession`, `CryptoKey`, `encrypt_sector`, `decrypt_sector`.
- `share_temppass` module for crypto-aware share flows.

## Usage

See `pcloud-daemon`'s `runtime.rs` for the canonical integration. Direct use
from outside the workspace is not supported yet.

## Features

None.

## Security posture

- Constant-time comparisons through `subtle`.
- All key material is zeroized on drop.
- No plaintext key is ever persisted to disk by this crate.

## License

Dual-licensed under `MIT OR Apache-2.0`.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

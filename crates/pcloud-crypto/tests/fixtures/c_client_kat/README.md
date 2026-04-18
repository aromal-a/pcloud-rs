# c_client_kat — cross-client Known Answer Test fixtures

## Provenance

These fixtures **lock the Rust pcloud-crypto sector wire format** so any
future change to key derivation, AAD binding, frame layout, or AEAD
choice must update both these bytes and an explicit fixture revision.

**Source:** generated from the AES-256-GCM + HMAC-SHA256(file-key/v1)
spec using the Python `cryptography` library (primitives-level, not via
this crate's own code). Generation script (reproducible):

```python
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
import hmac, hashlib, struct

master     = bytes([0x42] * 32)
file_seed  = bytes([0xAB] * 32)
nonce      = bytes([0xCD] * 12)      # fixed (KAT-only; not from OsRng)
sector_idx = 0
plaintext  = b"known-answer-test-plaintext-payload-12345678"

# Derive per-file key: HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)
h = hmac.new(master, digestmod=hashlib.sha256)
h.update(b"pcloud-crypto/file-key/v1")
h.update(file_seed)
file_key = h.digest()

aad   = struct.pack(">I", sector_idx)      # big-endian u32
frame = aad + nonce + AESGCM(file_key).encrypt(nonce, plaintext, aad)
```

The only non-byte-identical input the Rust runtime would supply is the
AES-GCM nonce (normally from `getrandom`); this fixture uses a fixed
`0xCD * 12` nonce so the ciphertext is reproducible.

## Cross-client status (bd-1du.10)

**This fixture does NOT prove compatibility with the legacy C client
(`pclsync/pcryptofolder.c`).** The C client's sector format, key
derivation label, AAD width/endianness, and AEAD choice have not been
independently captured into a cross-client KAT. Cross-client compatibility
is tracked under `bd-1du.10` and remains `Partial` in the parity matrix
(see `C_FEATURE_PARITY_MATRIX.csv`).

When a C-client-encrypted sector is captured, replace these files with
the C vectors and drop the fixed-nonce generation note.

## File layout

| File | Contents | Size |
|------|----------|------|
| `master_key.hex`           | Hex-encoded 32-byte master key (`0x42` repeated)   | 64 ASCII chars |
| `file_seed.hex`            | Hex-encoded 32-byte file seed (`0xAB` repeated)    | 64 ASCII chars |
| `sector.bin`               | Sealed sector frame: `[BE u32 index][12-byte nonce][ct || 16-byte tag]` | 76 bytes |
| `expected_plaintext.bin`   | Expected plaintext recovered by `open_sector`      | 44 bytes |

## What the test asserts

`tests/round_trip.rs::kat::kat_c_client_vector`:

1. Reads `master_key.hex` and `file_seed.hex`, decodes to 32 bytes each.
2. Reads `sector.bin` and `expected_plaintext.bin`.
3. Calls `pcloud_crypto::content::open_sector(&file_key, 0, &sector)`
   where `file_key = HMAC-SHA256(master, "pcloud-crypto/file-key/v1" || file_seed)`.
4. Asserts the decrypted plaintext byte-matches `expected_plaintext.bin`.

Any change to AAD endianness, per-file key derivation label, or frame
layout will make this test fail loudly with `AuthFailed` or length
mismatch — exactly the regression surface we want locked.

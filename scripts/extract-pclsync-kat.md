# Extract pclsync-compat KAT fixtures

Run this once (or re-run any time) to capture real ciphertext + key material
from your pCloud account into the Rust test-fixture directory. The extracted
blobs drive `crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs`, which is
the byte-level interop proof for the PclsyncCompat crypto backend.

## Prerequisites

### One-time account setup (via the pCloud **web UI**)

`pcloudcc` does not expose `crypto setup` or `mkdir` at runtime. Setup and
folder creation must happen in the official client once per account.

1. Make sure pCloud Crypto is active on your account. (It is, per session
   context — the account already passes unlock with the login password.)
2. Open <https://my.pcloud.com> and sign in.
3. Navigate to **Crypto Folder** (the top-level encrypted root).
4. Create a sub-folder called exactly `pclsync-kat-v1` (no quotes, no
   leading/trailing whitespace).
5. Upload the fixture file
   `crates/pcloud-crypto/tests/fixtures/pclsync_v2/kat-plaintext-v1.bin`
   into that sub-folder using the web UI's **Upload** button.
   - The file must end up at `Crypto Folder / pclsync-kat-v1 / kat-plaintext-v1.bin`.
   - Size on the server must show 4096 bytes. If it says anything else,
     the upload was corrupted — delete and re-upload.
   - The web UI encrypts client-side; what the pCloud servers store is
     the ciphertext we need to capture.

### Local environment

- `PCLOUD_USERNAME` and `PCLOUD_PASSWORD` set in your shell (direnv loads
  these from `.env` automatically).
- 2FA must be OFF (already the case in this session). If you re-enable it
  later, the extraction will fail at the `userinfo` call because we can't
  programmatically submit a TFA code.
- Python 3.9+ (stdlib only — no extra deps).

## Run

```bash
python3 scripts/extract-pclsync-kat.py
```

Expected output:

```
[*] plaintext sha256 = c8f5d0341d54d951a71b136e6e2afcb14d11ed8489a7ae126a8fee0df6ecf193
[*] authenticating as gestion.docbetry@gmail.com
[*] auth token acquired
[*] fetching crypto_getuserkeys
    priv_key_ver1 blob: NNN bytes
    pub_key_ver1 blob : NNN bytes
[*] locating crypto folder tree
    Crypto Folder folderid=NNN
    'pclsync-kat-v1' folderid=NNN
    'kat-plaintext-v1.bin' fileid=NNN size=4096
[*] fetching crypto_getfolderkey
    folder wrapped sym_key: NNN bytes
[*] fetching crypto_getfilekey
    file wrapped sym_key: NNN bytes
    file hash (u64): ...
[*] fetching getfilelink
    downloading https://...
    ciphertext: 4096 bytes (or 4128 / 4144 depending on pCloud's authentication overhead)

[+] all fixtures written to:
    crates/pcloud-crypto/tests/fixtures/pclsync_v2/
```

On success 6 files are written (plus `README.md`):

| File | Purpose |
|---|---|
| `kat-plaintext-v1.bin` | committed, the known-input fixture |
| `kat-priv-key-ver1.blob` | PBKDF2-wrapped RSA-4096 priv key |
| `kat-pub-key-ver1.blob` | user pubkey (unwrapped) |
| `kat-folder-sym-key-wrapped.bin` | RSA-OAEP-wrapped folder sym key |
| `kat-file-sym-key-wrapped.bin` | RSA-OAEP-wrapped file sym key |
| `kat-file-hash.txt` | pCloud file-hash (u64 decimal) |
| `kat-ciphertext-v1.bin` | server-served raw ciphertext |
| `README.md` | auto-generated provenance |

## Run the Rust KAT

```bash
export PCLOUD_KAT_PASSWORD='<your login password>'
cargo test -p pcloud-crypto --test pclsync_compat_kat_live -- --ignored
```

The test:

1. Loads `kat-plaintext-v1.bin`, computes expected SHA-256.
2. Parses `kat-priv-key-ver1.blob` → extracts PBKDF2 salt + encrypted RSA-DER.
3. Derives KEK from `PCLOUD_KAT_PASSWORD` + salt via
   `pclsync_kdf::derive_kek`.
4. Unwraps the encrypted RSA-DER with
   `pclsync_modes::aes256_ctr_pclsync_xor_inplace`.
5. Parses the DER into an `rsa::RsaPrivateKey`.
6. RSA-OAEP-unwraps `kat-file-sym-key-wrapped.bin` →
   `pclsync_rsa::SymKeyVer1`.
7. Splits `kat-ciphertext-v1.bin` into 4096-byte sectors + 32-byte tag per
   sector (+ Merkle tree tail if `file size > 4096`).
8. Calls `pclsync_sector::open_sector` for sector 0.
9. Asserts the decrypted plaintext SHA-256 matches the expected.

A successful run is the proof that pcloud-rs's PclsyncCompat backend
decrypts exactly what the official pCloud clients produce — **the
byte-level interop KAT** referenced by bead `pcloud-rs-s1p.13`.

## Caveats / troubleshooting

- **"userinfo did not return an 'auth' token"**: your account has 2FA on,
  or the login password is wrong, or the account is locked. Check
  <https://my.pcloud.com> by signing in through the browser.
- **"folder 'pclsync-kat-v1' not found"**: the web-UI folder wasn't
  created, or the name has a typo / invisible whitespace. Delete and
  recreate with the exact name above.
- **ciphertext size != 4096**: pCloud's direct download may include an
  authentication trailer (up to 32 bytes per sector + a small Merkle
  root). 4128 or similar sizes are normal — the Rust test handles them.
- **Re-running**: safe. The script overwrites all the extracted artefacts
  in place; the web-UI setup does NOT need to be redone.
- **Non-ASCII filenames (NFC/NFD)**: this KAT fixture uses an ASCII-only
  folder and filename (`pclsync-kat-v1` / `kat-plaintext-v1.bin`), so the
  open NFC/NFD normalization gap on macOS does NOT affect it. Cross-client
  compatibility for non-ASCII filenames is a separate open issue; see
  `docs/enterprise/crypto-compat.md` for the current caveat and the
  tracking bead under `bd-1du`.

## Cleanup (optional, after you're satisfied the KAT works)

The fixture folder on your pCloud account (`pclsync-kat-v1`) can be
deleted via the web UI any time. The fixture files committed to this
repo continue to drive the offline KAT test.

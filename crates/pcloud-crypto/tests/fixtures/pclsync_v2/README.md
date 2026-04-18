# pclsync-compat KAT fixtures

Extracted on 2026-04-18T18:21:34.055160+00:00 from https://eapi.pcloud.com using the account owner's login.

## Files

- `kat-plaintext-v1.bin` — committed, known input (4096 bytes, byte[i] = i % 256)
  sha256: `c8f5d0341d54d951a71b136e6e2afcb14d11ed8489a7ae126a8fee0df6ecf193`
- `kat-priv-key-ver1.blob` — RSA-4096 priv key wrapped with PBKDF2-HMAC-SHA512(login_password)
- `kat-pub-key-ver1.blob` — RSA-4096 public key (unwrapped)
- `kat-folder-sym-key-wrapped.bin` — RSA-OAEP-wrapped sym_key_ver1 for the KAT folder
- `kat-file-sym-key-wrapped.bin` — RSA-OAEP-wrapped sym_key_ver1 for the KAT file
- `kat-file-hash.txt` — pCloud file-hash (u64, decimal)
- `kat-ciphertext-v1.bin` — raw ciphertext as served by pCloud's direct-download endpoint

## Extraction metadata (from crypto_getuserkeys)

- iterations hint: ``
- salt hint (may be empty — salt lives inside priv_key_ver1 blob): ``

## Rust test flow

See `crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs`. Gated on
`PCLOUD_KAT_PASSWORD` env var (= the login password used during extraction).
Test is skipped unless both the env var and all fixture files are present.

## Re-running

`python3 scripts/extract-pclsync-kat.py` overwrites the extracted artefacts
in place. The manual web-UI setup (folder + plaintext upload) only needs to
happen once; after that, re-runs take a few seconds.

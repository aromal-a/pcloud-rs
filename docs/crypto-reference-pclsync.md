# pclsync Crypto Reference — Authoritative Spec

Scope: exact bit-level behavior of the legacy C pCloud Crypto Folder
("pclsync") scheme, as driven by `C_CODE/pclsync/pcrypto.{c,h}`,
`pcryptofolder.{c,h}`, `pfscrypto.{c,h}`, `psettings.h`, and `pssl.{c,h}`.
Every claim below is cited to `C_CODE/pclsync/<file>:<line>`. This
document is the source of truth for a Rust refactor that must produce
byte-identical ciphertext and server payloads.

## 0. Constants (the KAT seeds)

| Constant                                   | Value                      | Source                                              |
| ------------------------------------------ | -------------------------- | --------------------------------------------------- |
| `PSYNC_CRYPTO_PASS_TO_KEY_ITERATIONS`      | `20000`                    | `psettings.h:168`                                   |
| `PSYNC_CRYPTO_PBKDF2_SALT_LEN`             | `64` (bytes)               | `psettings.h:169`                                   |
| `PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN`         | `128`                      | `psettings.h:170`                                   |
| `PSYNC_CRYPTO_RSA_SIZE`                    | `4096` bits                | `psettings.h:171`                                   |
| `PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT` | `0`                    | `psettings.h:173`                                   |
| `PSYNC_CRYPTO_PUB_TYPE_RSA4096`            | `0`                        | `psettings.h:174`                                   |
| `PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC`     | `0`                        | `psettings.h:175`                                   |
| `PSYNC_CRYPTO_SYM_FLAG_ISDIR`              | `1`                        | `pcryptofolder.h:44`                                |
| `PSYNC_CRYPTO_SECTOR_SIZE`                 | `4096` (bytes, plaintext)  | `pcryptofolder.h:46`                                |
| `PSYNC_CRYPTO_AUTH_SIZE`                   | `32` (= 2 × AES block)     | `pcrypto.h:37`                                      |
| `PSYNC_CRYPTO_HASH_TREE_SECTORS`           | `128` (= 4096 / 32)        | `pfscrypto.h:41`                                    |
| `PSYNC_CRYPTO_MAX_HASH_TREE_LEVEL`         | `6`                        | `pcrypto.h:39`                                      |
| `PSYNC_CRYPTO_FLAG_TEMP_PASS`              | `1`                        | `psynclib.h:275`                                    |
| `PSYNC_AES256_KEY_SIZE`                    | `32`                       | `pssl.h:50`                                         |
| `PSYNC_AES256_BLOCK_SIZE`                  | `16`                       | `pssl.h:49`                                         |

There is **no** HKDF, no Argon2, no SHA-3, no GCM anywhere in the C
pclsync crypto path.

## 1. Key Hierarchy

### 1.1 Password → KEK (and HMAC key)

```c
// pcryptofolder.c:380..386
pdbg_logf(D_NOTICE, "generating salt");
pssl_rand_strong(salt, PSYNC_CRYPTO_PBKDF2_SALT_LEN);
aeskey = pssl_derive_key_sha512(
    password, PSYNC_AES256_KEY_SIZE + PSYNC_AES256_BLOCK_SIZE, salt,
    PSYNC_CRYPTO_PBKDF2_SALT_LEN, PSYNC_CRYPTO_PASS_TO_KEY_ITERATIONS);
enc = pcrypto_ctr_encdec_create(aeskey);
```

- KDF: **PBKDF2-HMAC-SHA512** (`pssl_derive_key_sha512`).
- Salt: 64 bytes from the OS CSPRNG (`pssl_rand_strong`).
- Iterations: **20 000** (fixed in `psettings.h:168`).
- Output length: `PSYNC_AES256_KEY_SIZE + PSYNC_AES256_BLOCK_SIZE` = 48
  bytes. First 32 bytes = AES-256 key; last 16 bytes = IV/counter prefix
  (see §2.0 and `pcrypto_ctr_encdec_create` at `pcrypto.c:169..183`).

### 1.2 KEK wraps the RSA private key

```c
// pcryptofolder.c:429..431
pdbg_logf(D_NOTICE, "encoding private key");
pcrypto_ctr_encdec_decode(enc, rsaprivatebin->data,
                          rsaprivatebin->datalen, 0);
```

- Cipher: **AES-256 in CTR mode** (`pcrypto_ctr_encdec_decode`,
  `pcrypto.c:192..244`). Counter = `dataoffset / 16`, serialized
  big-endian into the top 8 bytes of the block, XOR'd with the 16-byte
  IV taken from the tail of the PBKDF2 output. There is **no
  authenticated-encryption tag** on the wrapped RSA private key.
- The wrapped blob is encoded into a versioned struct `priv_key_ver1`
  (`pcryptofolder.c:74..78`): `{ uint32_t type; uint32_t flags;
  unsigned char salt[64]; unsigned char key[] }` with
  `type = PSYNC_CRYPTO_TYPE_RSA4096_64BYTESALT_20000IT = 0`. Base64 of
  that struct is uploaded as `privatekey` to `crypto_setuserkeys`
  (`pcryptofolder.c:155..168`, `pcryptofolder.c:341..349`).
- The RSA key itself is 4096-bit, generated via
  `pssl_gen_rsa(PSYNC_CRYPTO_RSA_SIZE)` (`pcryptofolder.c:393`).

### 1.3 RSA unwraps per-folder / per-file symmetric keys

```c
// pssl.c:718..739 (prsa_encrypt_data)
if ((code = mbedtls_rsa_rsaes_oaep_encrypt(
         rsa, rng_get, &rng,
         NULL, 0, datalen, data, ret->data))) { ... }
```

- Padding: **RSA-OAEP** with the mbedTLS default hash
  (SHA-1, per mbedTLS when `hash_id` on the RSA context is left at the
  default set by `mbedtls_rsa_init`), empty label (`NULL, 0`).
  This is the single wrap/unwrap primitive for every `folderkey` and
  `filekey` returned by the server.
- Decrypt counterpart: `mbedtls_rsa_rsaes_oaep_decrypt`
  (`pssl.c:742..758`). Used at `pcryptofolder.c:511`, `:975`, `:1003`,
  `:1143`, `:1199`, `:1428`, `:1526` to recover folder and file
  symmetric-key blobs.

### 1.4 Per-folder / per-file symmetric blob: `sym_key_ver1`

```c
// pcryptofolder.c:86..90
  uint32_t type;
  uint32_t flags;
  unsigned char aeskey[PSYNC_AES256_KEY_SIZE];        // 32
  unsigned char hmackey[PSYNC_CRYPTO_HMAC_SHA512_KEY_LEN]; // 128
```

So each encrypted folder/file carries **160 bytes of raw key material**
plus two 4-byte fields: a 32-byte AES-256 key, a 128-byte HMAC-SHA512
key. `type = PSYNC_CRYPTO_SYM_AES256_1024BIT_HMAC = 0`
(`psettings.h:175`). The `hmackey` doubles as both the PRF for the
sector-auth HMAC and as the CBC-style IV source (it is appended
verbatim after the 32-byte AES key in the `psync_symmetric_key_t` buffer
used to build the encdec object; see `pcrypto_sec_encdec_create` at
`pcrypto.c:444..466`, where `ivlen = keylen - 32`).

Per-file keys are generated by `pcryptofolder_filencoder_key_new`:
server stores the RSA-OAEP-wrapped blob, and every file download
re-unwraps it with the user's RSA private key (see "filekey" in the
`crypto_getfilekey` response, `pcryptofolder.c:879`).

## 2. Sector encryption (content) — `pcrypto_encode_sec`

This is **not** AES-CTR and it is **not** AES-GCM. It is a custom
AEAD with ciphertext-stealing CBC body and a 32-byte authenticator.

```c
// pcrypto.c:487..512  — encode_sec prologue (quoted verbatim, 10 lines)
void pcrypto_encode_sec(
    pcrypto_sector_encdec_t enc, const unsigned char *data,
    size_t datalen, unsigned char *out, pcrypto_sector_auth_t authout,
    uint64_t sectorid) {
  psync_hmac_sha512_ctx ctx;
  unsigned char buff[PSYNC_AES256_BLOCK_SIZE * 3],
      hmacsha1bin[PSYNC_SHA512_DIGEST_LEN], rnd[PSYNC_AES256_BLOCK_SIZE];
  pdbg_assert(PSYNC_CRYPTO_AUTH_SIZE == 2 * PSYNC_AES256_BLOCK_SIZE);
  pssl_rand_strong(rnd, PSYNC_AES256_BLOCK_SIZE);
  psync_hmac_sha512_init(&ctx, enc->iv, enc->ivlen);
  psync_hmac_sha512_update(&ctx, data, datalen);
```

Step by step:

1. Generate `rnd` — 16 random bytes per sector
   (`pcrypto.c:499`, `pssl_rand_strong`).
2. Compute `tweak = HMAC-SHA512(hmackey, plaintext || sectorid_le64 ||
   rnd)`, truncated to the first 16 bytes
   (`pcrypto.c:500..504`). `enc->iv` is the 128-byte `hmackey` from
   §1.4.
3. Short-sector branch (`datalen < 16`, `pcrypto.c:505..512`):
   ciphertext = `rnd XOR plaintext` (only `datalen` bytes copied to
   `out`). Auth tag = `AES256-ECB(aes_key, rnd || tweak)` (2 blocks,
   encrypted with `psync_aes256_encode_2blocks_consec`).
4. Long-sector branch (`datalen >= 16`, `pcrypto.c:514..559`):
   - Auth tag construction (`pcrypto.c:519..525`): the 32-byte auth
     field is `AES256-ECB(aes_key, [rnd[0..8] || tweak[0..16] ||
     rnd[8..16]])` — i.e., the tweak is sandwiched between the two
     halves of `rnd`.
   - Body is encrypted in **CBC mode with `tweak` as the initial IV**
     (`pcrypto.c:526..550`), over full 16-byte blocks.
   - If `datalen % 16 != 0`, the last two blocks use **CBC
     ciphertext-stealing (CS3 ordering)** (`pcrypto.c:551..559`).
5. Sector ciphertext size on disk == plaintext size (no padding). The
   32-byte auth tag is stored **separately** in the hash tree (§3).

Decoding (`pcrypto.c:562..642`) reverses the body, then recomputes
`HMAC-SHA512(hmackey, plaintext || sectorid_le64 || decrypted_tweak)`
and compares against the second half of the decrypted auth tag via
`memcmp_const` (`pcrypto.c:640`). Tag mismatch returns non-zero.

The `sector_encdec` object requires only `PSYNC_AES256_KEY_SIZE` from
the sym key; `ivlen` is whatever remains (normally `hmackey_len = 128`)
and is used directly as HMAC-SHA512 key
(`pcrypto.c:444..466`).

## 3. Sector framing and authentication tree (pfscrypto.c)

**Sector size.** Exactly `PSYNC_CRYPTO_SECTOR_SIZE = 4096` plaintext
bytes (`pcryptofolder.h:46`). Ciphertext length matches plaintext
(no padding — ciphertext-stealing CBC preserves length).

**Sector layout on the wire.** A file's ciphertext stream is packed
sectors 0..N−1 in order, then per-level auth tables. Offsets are
computed by `pfs_crpt_offset_by_size` (`pfscrypto.c:135..195`). The
`psync_crypto_offsets_t` struct (`pcrypto.h:41..49`) carries:

- `plainsize` (64-bit plaintext length),
- per-tree-level last-auth-sector offsets/lengths
  (`lastauthsectoroff[level]`, `lastauthsectorlen[level]`),
- `masterauthoff` — offset of the final master auth block.

**Authentication tree.** `PSYNC_CRYPTO_HASH_TREE_SECTORS = 128` leaves
per internal node (`pfscrypto.h:41`). Up to
`PSYNC_CRYPTO_MAX_HASH_TREE_LEVEL = 6` levels
(`pcrypto.h:39`). Level-0 auths are the 32-byte tags from
`pcrypto_encode_sec`; higher-level auths are formed by
`pcrypto_sign_sec` (`pcrypto.c:644..654`): `AES256-ECB(aes_key,
HMAC-SHA512(hmackey, level_block)[0..32])`. The last level produces
a single 32-byte master tag written at `masterauthoff`
(`pfscrypto.c:693..713`).

Files ≤ 4096 bytes have `needmasterauth = 0` and store only the leaf
auth tag (`pfscrypto.c:175..181`).

**Integrity model — explicit:**

- Per-sector: **yes**, 32-byte tag over (plaintext, sector_id, rnd).
- Per-file: **yes**, Merkle-like tree rooted at `masterauthoff`.
- Per-folder / cross-file: **no** signature; folders are authenticated
  only by possession of the RSA-wrapped folder key.
- Server swap of a whole ciphertext with a matching master tag from a
  different file cannot be detected unless the caller already knows
  the expected master tag. The hash tree binds sectors to a file's
  own root, not to the filename or fileid.

## 4. Filename encoding (`pcryptofolder.c`)

```c
// pcryptofolder.c:1350..1360  — fldencode_filename
char * pcryptofolder_fldencode_filename(pcrypto_textenc_t encoder,
                                   const char *name) {
  unsigned char *filenameenc, *filenameb32;
  size_t filenameenclen;
  pcrypto_encode_text(encoder, (const unsigned char *)name,
                                  strlen(name), &filenameenc, &filenameenclen);
  filenameb32 =
      putil_base32_encode(filenameenc, filenameenclen, &filenameenclen);
  return (char *)filenameb32;
}
```

- Primitive: **`pcrypto_encode_text`** (`pcrypto.c:273..311`), a
  *non-deterministic* AES-256-CBC-like encoding with an HMAC-SHA512
  tweak derived from the last blocks of plaintext:
  - Plaintext length ≤ 16: output = `AES256-ECB(aes_key, plaintext_padded
    XOR iv_first_16)` — one block.
  - Longer: compute `tweak = HMAC-SHA512(hmackey, plaintext[16..])`
    (truncated to 16 bytes), then first block = `AES-ECB(aes_key,
    plaintext[0..16] XOR tweak)`; subsequent blocks are CBC-chained
    from the previous ciphertext block (`pcrypto.c:294..310`).
- Output encoding: **base32** (`putil_base32_encode`,
  `pcryptofolder.c:1357`). Decoding mirror: `pcryptofolder.c:1286..1291`
  — base32-decode then `pcrypto_decode_text`.
- Length: plaintext is padded up to a 16-byte multiple
  (`ALIGN_A256_BS(txtlen)`), then base32-expanded 8:5 — so for an
  N-byte name the encoded length is `ceil(ceil(N/16)*16 * 8/5)` ASCII
  chars. There is **no explicit maximum length** in the C source;
  server-side limits apply.
- Determinism: **no.** The HMAC tweak is over plaintext, so two
  identical names encode to the *same* ciphertext within one folder
  (given fixed key/iv) — but different folders use different keys
  (per-folder RSA-wrapped sym key) so cross-folder lookup requires
  re-encoding. The code relies on per-folder determinism for directory
  listing.

## 5. File-level operation flow

- **Upload:** client reads plaintext → chunks into 4096-byte sectors →
  `pcrypto_encode_sec` per sector → appends per-level auth blocks and
  master tag → uploads the raw byte stream via the normal `upload_*`
  API. The per-file sym key (RSA-OAEP-wrapped under the user pubkey)
  is sent separately via `crypto_*` endpoints that set the `filekey`.
- **Download:** server returns the raw ciphertext stream (same length
  as plaintext plus auth overhead). Client calls `crypto_getfilekey`
  (`pcryptofolder.c:879`) to fetch the wrapped `filekey`, RSA-OAEP
  unwraps it, rebuilds the `sector_encdec`, then decodes sector by
  sector; tag mismatch aborts the read.
- **Directory listing:** server returns encrypted names (base32 of
  CBC-tweaked AES output). Client either decodes them with
  `pcryptofolder_flddecode_filename` (needs `folderkey`), or
  *looks up a specific plaintext name* by encoding it locally and
  comparing — the encoding is deterministic under a fixed folder key.

## 6. Server / wire surface

`papi_send2` method names invoked from `pcryptofolder.c`:

- `crypto_setuserkeys` (`:168`) — upload base64(`priv_key_ver1`),
  base64(`pub_key_ver1`), hint, expire.
- `crypto_getuserkeys` (`:223`, `:307`) — returns `privatekey`,
  `publickey` (base64 strings, per `:247..252`).
- `crypto_getuserhint` (`:467`).
- `crypto_reset` (`:739`).
- `crypto_getfolderkey` (`:826`).
- `crypto_getfilekey` (`:879`).

The user-key row is persisted locally to the `setting` table with
these ids (`pcryptofolder.c:124..142`): `crypto_private_key`,
`crypto_public_key`, `crypto_private_salt`, `crypto_private_iter`,
`crypto_public_sha1`, `crypto_private_sha1`, `crypto_private_flags`.

Response fields the client parses: `privatekey`, `publickey`,
`folderkey`, `filekey` (all base64 blobs).

## 7. Deltas vs current Rust implementation

| Step                             | C behavior                                            | Rust today (`crates/pcloud-crypto/src/*`)                                                                                   | Delta severity                                    |
| -------------------------------- | ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| Password KDF                     | **PBKDF2-HMAC-SHA512, 20000 iters, 64-byte salt**     | `keys.rs:1..22` — **Argon2id, 16-byte salt**                                                                                | **Wire-incompatible.** Cannot unwrap server RSA.  |
| Wrapped RSA private key          | AES-256-CTR, `priv_key_ver1` struct, base64           | **Not implemented** (no RSA path, no `priv_key_ver1`)                                                                       | Missing entire primitive.                         |
| Sym-key wrap (folder/file)       | RSA-4096-OAEP over `sym_key_ver1` (4+4+32+128 bytes)  | **Not implemented**                                                                                                         | Missing.                                          |
| Sector encryption                | Custom CBC-CS + HMAC-SHA512 tweak + 32-byte auth tag  | `content.rs:21..90` — **AES-256-GCM with 12-byte nonce, 16-byte tag, stored inline with 4-byte sector index prefix**        | **Wire-incompatible.** Ciphertext layout differs. |
| Sector size                      | 4096 plaintext, no padding                            | `content.rs:32` — `SECTOR_SIZE_BYTES=4096` ✓; but overhead `SECTOR_OVERHEAD=32` per sector (C has 0 inline, 32 in tree)     | Layout differs.                                   |
| Authentication tree              | 128-way Merkle, up to 6 levels, master tag            | **Not implemented.** Rust has inline GCM tags only.                                                                         | Missing.                                          |
| Filename encoding                | CBC-CS AES + HMAC-SHA512 tweak + **base32**           | `metadata.rs:54..61` — **HMAC-SHA256 hex, one-way only, not invertible**                                                    | **Wire-incompatible and not reversible.**         |
| Server API                       | `crypto_setuserkeys` / `crypto_getuserkeys` / …       | `lib.rs` issues some crypto control IPC but does **not** implement the wire methods above                                   | Missing surface.                                  |
| Share temp-pass                  | AES-CTR-wrapped temp RSA key (see `share_temppass.rs`)| `share_temppass.rs` present but depends on same Rust primitives above                                                       | Blocked on upstream primitives.                   |
| `priv_key_flags`                 | `PSYNC_CRYPTO_FLAG_TEMP_PASS = 1`                     | `keys.rs:76..80` — defines `PRIV_KEY_FLAG_TEMP_PASS`, matches value                                                         | OK.                                               |

## 8. Test-vector seeds (to lock refactor to C)

1. `PBKDF2-HMAC-SHA512(password="test", salt=64×0x00, iters=20000, dkLen=48)`
   → first 32 bytes = AES key, last 16 = IV. (C: `pssl_derive_key_sha512`
   via `pcryptofolder.c:383..385`.)
2. `pcrypto_ctr_encdec_decode` with `dataoffset=0` over a known
   32-byte plaintext → matches manual AES-256-CTR with
   counter=`be64(0)` XOR `IV[8..16]`, block-counter starting at 0
   (`pcrypto.c:192..244`).
3. `pcrypto_encode_sec` with `sectorid=0`, `hmackey=128×0x01`,
   `aes_key=32×0x02`, `plaintext=16×0x03`, `rnd=16×0x04`:
   - `tweak = HMAC-SHA512(128×0x01, [16×0x03, 0x00×8 sectorid,
     16×0x04])[0..16]`
   - auth = `AES-256-ECB(32×0x02, rnd[0..8] || tweak || rnd[8..16])`
   - ciphertext body = `AES-256-CBC(32×0x02, IV=tweak, 16×0x03)`
4. Filename encode of `"hello.txt"` (9 bytes, padded to 16) with
   `aes_key=32×0x02`, `hmackey=128×0x01`, `iv[0..16]=16×0x05`:
   - plaintext ≤ 16 branch: output = `AES-256-ECB(aes_key,
     pad("hello.txt", 16) XOR 16×0x05)` → base32-encode to 26 chars.
5. `sym_key_ver1` with `type=0 flags=0 aeskey=32×0x06 hmackey=128×0x07`
   → 168-byte struct, RSA-OAEP-wrapped under the user pubkey produces
   a `folderkey` byte string of length `rsalen = 512` bytes
   (`pssl.c:723`, `pssl.c:737`).

## 9. Summary of invariants a Rust refactor MUST preserve

1. **PBKDF2-HMAC-SHA512 / 20000 iters / 64-byte salt** is
   non-negotiable — this is what the server's stored `privatekey`
   envelope was built against.
2. **Sector cipher is CBC-CS + HMAC-SHA512 tweak + 32-byte external
   auth tag**, not GCM. Ciphertext length == plaintext length. Tags
   live in a separate 128-ary Merkle tree appended to the file.
3. **Wrapping primitive is RSA-4096-OAEP** for every `folderkey`,
   `filekey`, and wrapped-sym-key blob that crosses the wire.
4. **Filenames are CBC-tweak AES then base32**, deterministic within a
   folder. Not HMAC-hex.
5. **Key blobs are versioned C-structs** (`priv_key_ver1`,
   `pub_key_ver1`, `sym_key_ver1`) with fixed offsets — the base64
   payload layout is part of the wire contract.

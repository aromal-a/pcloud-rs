# extract-pclsync-kat — Human Walkthrough

This document is the step-by-step companion to `scripts/extract-pclsync-kat.sh`.
Read it in full before you run the script.

---

## What this does

The script extracts real C-client-produced ciphertext and key material from a
live pCloud account into `crates/pcloud-crypto/tests/fixtures/pclsync_v2/`.
Those files are used by the Wave 1 Rust KAT tests to verify that the Rust
crypto primitives produce byte-identical output to the official pclsync
implementation.

---

## One-way operation — read this first

**Crypto setup on a pCloud account is one-way.**

Once you run `crypto setup` via `pcloudcc`, pCloud generates a key pair on
the server bound to the password you supply.  You cannot undo this without
contacting pCloud support.  If your account already has crypto enabled with a
real password you care about, **do not run this script** — it will attempt to
set up crypto again and will likely fail or corrupt your existing setup.

Options if your account already has crypto:

1. Use a **dedicated throwaway pCloud account** for the KAT extraction.
   This is the recommended approach.
2. If you have already run `crypto setup` with the KAT password on a
   dedicated account, skip Step 7 (crypto setup) in the script and go
   straight to the folder creation step.

---

## Prerequisites

Before running the script, verify:

1. **pcloudcc binary** is at the repo root and is executable:

   ```
   ls -l ./pcloudcc
   chmod +x ./pcloudcc
   ```

2. **.env file** exists at the repo root and contains your pCloud credentials
   for the KAT account (not your personal account):

   ```
   PCLOUD_USERNAME=your-kat-account@example.com
   PCLOUD_PASSWORD=your-login-password
   ```

   If you use direnv, `direnv allow` in the repo root is sufficient.
   Otherwise: `source .env`

3. **sqlite3** is installed:

   ```
   sqlite3 --version
   ```

4. **python3** is installed (used for byte manipulation):

   ```
   python3 --version
   ```

5. **curl** is installed.

6. **No pcloudcc daemon is already running** for this account:

   ```
   pkill pcloudcc || true
   ```

---

## The KAT password

The script uses this fixed, public password for all crypto operations:

```
pclsync-kat-fixture-v1-do-not-use-for-real
```

This password is intentionally public and embedded in the script.  It is not
a secret.  The fixtures it produces are safe to commit because:

- The wrapped private key blob is useless without this password.
- This password must never be used for any real data.
- The plaintext content of all fixture files is entirely synthetic.

---

## Step-by-step run guide

### 1. Open two terminal windows

Terminal A: the script (`bash scripts/extract-pclsync-kat.sh`)
Terminal B: interactive pcloudcc commands

### 2. Terminal A — start the script

```bash
cd /path/to/pcloud-rs
source .env        # if not using direnv
bash scripts/extract-pclsync-kat.sh
```

The script will verify the binary, credentials, and create the fixture
directory.  It will then pause and instruct you to act in Terminal B.

### 3. Terminal B — start the daemon

When the script prompts you, run in Terminal B:

```bash
./pcloudcc --username $PCLOUD_USERNAME --password --daemonize
```

Enter your pCloud login password when prompted.  Wait until you see a
message indicating login success (e.g., "pCloud account logged in").

Return to Terminal A and press ENTER.

### 4. Terminal B — crypto setup

When the script prompts for crypto setup, run in Terminal B:

```bash
./pcloudcc --commands_only
```

At the `pcloudcc>` prompt:

```
crypto setup
```

When asked for the crypto password, type **exactly**:

```
pclsync-kat-fixture-v1-do-not-use-for-real
```

Confirm it when prompted again.  Type `exit` or press Ctrl-D to leave the
interactive session.

Return to Terminal A and press ENTER.

### 5. Terminal B — create and mark the KAT folder

The script will print the folder name it generated (e.g., `kat-fixture-v1-20260418-123456`).
In Terminal B:

```bash
./pcloudcc --commands_only
```

At the prompt:

```
mkdir /kat-fixture-v1-<timestamp>
crypto folder /kat-fixture-v1-<timestamp>
exit
```

Return to Terminal A and press ENTER.

### 6. Terminal B — upload the plaintext files

When prompted, in Terminal B:

```bash
./pcloudcc --commands_only
```

At the prompt (use the exact paths printed by the script):

```
put crates/pcloud-crypto/tests/fixtures/pclsync_v2/c_client_sector_id_0_plaintext.bin /kat-fixture-v1-<timestamp>/kat_4096.bin
put crates/pcloud-crypto/tests/fixtures/pclsync_v2/c_client_sector_id_1_plaintext.bin /kat-fixture-v1-<timestamp>/kat_5000.bin
exit
```

Wait for each upload to complete before exiting.

Return to Terminal A and press ENTER.

### 7. Automated extraction

From this point, the script runs automatically.  It will:

- Fetch the auth token from the pcloudcc SQLite database
- Call `listfolder` to get file IDs
- Download ciphertext blobs via `getfilelink`
- Call `crypto_getuserkeys` to extract the wrapped private key and PBKDF2 salt
- Call `crypto_getfolderkeys` to extract the RSA-OAEP-wrapped folder sym key
- Write all files to `crates/pcloud-crypto/tests/fixtures/pclsync_v2/`
- Generate `README.md` in the fixture directory

If the auth token cannot be found automatically, the script will prompt you
to paste it.  You can retrieve it by running:

```bash
./pcloudcc --commands_only
# then at the prompt: token
```

---

## After the script completes

### Check the fixture directory

```bash
ls -lh crates/pcloud-crypto/tests/fixtures/pclsync_v2/
```

You should see all files listed in the fixture README.

### Fill the placeholder files (Wave 1 task)

Three files are placeholders that Wave 1 must fill after implementing the
private-key decryption path:

- `c_client_filename_hello_txt_aes_key.bin`
- `c_client_filename_hello_txt_hmac_key.bin`
- `c_client_filename_hello_txt.b32`

The derivation path is:

```
PBKDF2-SHA512(KAT_CRYPTO_PASSWORD, salt, 20000, 32) → priv_key_aes_key
AES-256-CBC-decrypt(priv_key_wrapped, priv_key_aes_key) → rsa_private_key
RSA-OAEP-decrypt(rsa_private_key, sym_key_wrapped) → folder_sym_key
folder_sym_key[0:32]  → AES key for filename enc
folder_sym_key[32:64] → HMAC key for filename enc
```

### Commit the fixtures

```bash
git add crates/pcloud-crypto/tests/fixtures/pclsync_v2/
git commit -m "feat(crypto): add pclsync_v2 KAT fixture files from C client extraction"
```

Do NOT commit:
- `.env`
- `./pcloudcc` (binary)
- Any file containing your login password

---

## Troubleshooting

### "crypto_getuserkeys returned empty privatekey field"

The API field name may differ by API version.  Run:

```bash
curl "https://api.pcloud.com/crypto_getuserkeys?auth=YOUR_TOKEN" | python3 -m json.tool
```

and find the correct field name, then pass the base64 value to:

```bash
python3 -c "import base64; open('path/to/file', 'wb').write(base64.b64decode('YOUR_B64'))"
```

### "listfolder failed" or "folder not found"

The upload in step 6 may not have completed.  Wait a few seconds and retry.
You can also verify in the pCloud web UI.

### pcloudcc hangs on login

Check that no other pcloudcc instance is running:

```bash
pkill pcloudcc
```

Then retry step 3.

---

## Security checklist

- [ ] I am using a dedicated KAT account, not my personal pCloud account.
- [ ] I understand crypto setup is one-way per account.
- [ ] The KAT password (`pclsync-kat-fixture-v1-do-not-use-for-real`) is
      public and I have not used it for any real data.
- [ ] My login password is not committed to any fixture file.
- [ ] I will not commit `.env` or the `pcloudcc` binary.

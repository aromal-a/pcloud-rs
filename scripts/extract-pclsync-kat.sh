#!/usr/bin/env bash
# =============================================================================
# extract-pclsync-kat.sh
#
# PURPOSE
#   Walk the user through extracting real C-client-produced ciphertext and
#   key material from a live pCloud account (via pcloudcc) into a committable
#   KAT fixture directory.
#
# USAGE
#   Read scripts/extract-pclsync-kat.md FIRST.
#   Then:   bash scripts/extract-pclsync-kat.sh
#
# SAFETY CONTRACT
#   - Uses a fixed, non-secret KAT password.  Your real crypto password is
#     never written to disk or printed.
#   - The wrapped private-key blob is bound to KAT_CRYPTO_PASSWORD only; no
#     real account secret is embedded in the fixture.
#   - All fixture content is either synthetic plaintext or key material that
#     is only useful with the KAT password (which is public and in this file).
#   - Fixture files carry a "FIXTURE — NOT FOR PRODUCTION" label in README.
#
# PRECONDITIONS
#   1. pcloudcc binary at repo root (./pcloudcc) and executable.
#   2. .env file containing PCLOUD_USERNAME and PCLOUD_PASSWORD (direnv loads
#      them automatically; otherwise source it manually).
#   3. pcloudcc daemon is NOT already running.
#   4. You have NOT previously set up crypto on this account — or you are
#      willing to reset it.  Crypto setup is ONE-WAY per account.
#      See scripts/extract-pclsync-kat.md for the full caveat.
# =============================================================================

set -euo pipefail

# ---------------------------------------------------------------------------
# Constants — do not change without updating the fixture README
# ---------------------------------------------------------------------------
KAT_CRYPTO_PASSWORD='pclsync-kat-fixture-v1-do-not-use-for-real'
KAT_FOLDER_PREFIX="kat-fixture-v1"
KAT_PLAINTEXT_CONTENT_4K="KAT VECTOR 1: AES-256-CBC-CTS via pclsync, 4096 bytes of 0x41"
FIXTURE_DIR="crates/pcloud-crypto/tests/fixtures/pclsync_v2"
PCLSYNC_DB="${HOME}/.pcloud/data.db"
PCLSYNC_CACHE="${HOME}/.pcloud"

BOLD='\033[1m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
RESET='\033[0m'

# ---------------------------------------------------------------------------
# Helper utilities
# ---------------------------------------------------------------------------
info()    { echo -e "${CYAN}[INFO]${RESET}  $*"; }
ok()      { echo -e "${GREEN}[OK]${RESET}    $*"; }
warn()    { echo -e "${YELLOW}[WARN]${RESET}  $*"; }
fatal()   { echo -e "${RED}[FATAL]${RESET} $*" >&2; exit 1; }
step()    { echo -e "\n${BOLD}===> $*${RESET}"; }
pause()   {
    echo -e "\n${YELLOW}--- Press ENTER when done, or Ctrl-C to abort ---${RESET}"
    read -r _
}

# ---------------------------------------------------------------------------
# Step 0: Locate repo root
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"
info "Repo root: ${REPO_ROOT}"

# ---------------------------------------------------------------------------
# Step 1: Verify pcloudcc
# ---------------------------------------------------------------------------
step "Step 1 — Verify pcloudcc binary"

PCLOUDCC="${REPO_ROOT}/pcloudcc"
if [[ ! -f "${PCLOUDCC}" ]]; then
    fatal "pcloudcc not found at ${PCLOUDCC}.  Build or place the binary there first."
fi
if [[ ! -x "${PCLOUDCC}" ]]; then
    fatal "pcloudcc is not executable.  Run: chmod +x ${PCLOUDCC}"
fi

PCLOUDCC_VERSION="$("${PCLOUDCC}" --version 2>&1 || true)"
ok "pcloudcc found.  Version output: ${PCLOUDCC_VERSION:-<none>}"

# ---------------------------------------------------------------------------
# Step 2: Verify credentials
# ---------------------------------------------------------------------------
step "Step 2 — Verify .env credentials"

ENV_FILE="${REPO_ROOT}/.env"
if [[ ! -f "${ENV_FILE}" ]]; then
    fatal ".env file not found at ${ENV_FILE}"
fi

# Try to load if not already set
if [[ -z "${PCLOUD_USERNAME:-}" ]]; then
    # shellcheck disable=SC1090
    source "${ENV_FILE}" 2>/dev/null || true
fi
if [[ -z "${PCLOUD_USERNAME:-}" ]]; then
    fatal "PCLOUD_USERNAME is not set.  Check your .env file."
fi
if [[ -z "${PCLOUD_PASSWORD:-}" ]]; then
    fatal "PCLOUD_PASSWORD is not set.  Check your .env file."
fi
ok "Credentials found for: ${PCLOUD_USERNAME}"

# ---------------------------------------------------------------------------
# Step 3: Prepare fixture directory
# ---------------------------------------------------------------------------
step "Step 3 — Prepare fixture directory"

mkdir -p "${REPO_ROOT}/${FIXTURE_DIR}"
ok "Fixture dir: ${REPO_ROOT}/${FIXTURE_DIR}"

# ---------------------------------------------------------------------------
# Step 4: Generate known-plaintext files locally (no pcloudcc needed yet)
# ---------------------------------------------------------------------------
step "Step 4 — Generate known-plaintext files"

# 4096-byte file: header string + 0x41 padding
PT_4K="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sector_id_0_plaintext.bin"
{
    printf '%s' "${KAT_PLAINTEXT_CONTENT_4K}"
    # pad with 0x41 ('A') to exactly 4096 bytes
    header_len=${#KAT_PLAINTEXT_CONTENT_4K}
    pad_len=$((4096 - header_len))
    python3 -c "import sys; sys.stdout.buffer.write(b'\\x41' * ${pad_len})"
} > "${PT_4K}"
actual_size=$(wc -c < "${PT_4K}")
if [[ "${actual_size}" -ne 4096 ]]; then
    fatal "4K plaintext file is ${actual_size} bytes, expected 4096"
fi
ok "4K plaintext written: ${PT_4K}"

# 5000-byte file (2-sector case): same header + 0x42 ('B') padding
PT_5K="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sector_id_1_plaintext.bin"
{
    printf '%s' "${KAT_PLAINTEXT_CONTENT_4K}"
    header_len=${#KAT_PLAINTEXT_CONTENT_4K}
    pad_len=$((5000 - header_len))
    python3 -c "import sys; sys.stdout.buffer.write(b'\\x42' * ${pad_len})"
} > "${PT_5K}"
ok "5K plaintext written: ${PT_5K} ($(wc -c < "${PT_5K}") bytes)"

# ---------------------------------------------------------------------------
# Step 5: Explain what comes next (interactive)
# ---------------------------------------------------------------------------
step "Step 5 — Pre-flight explanation"

cat <<'MSG'

  This script cannot run pcloudcc for you because crypto-setup requires
  interactive confirmation.  From this point on you will:

    a) Start pcloudcc in daemon mode
    b) Use pcloudcc --commands_only to set up crypto with the KAT password
    c) Create a test folder and upload the plaintext files
    d) Let this script extract the ciphertext and key blobs

  The KAT_CRYPTO_PASSWORD this script uses is:

      pclsync-kat-fixture-v1-do-not-use-for-real

  This is public and lives in this script.  Do NOT use it for real crypto.

MSG

echo -e "${YELLOW}WARNING: Crypto setup is one-way per account.  If you have already set"
echo -e "up crypto on ${PCLOUD_USERNAME} with a real password you care about, STOP NOW."
echo -e "See scripts/extract-pclsync-kat.md for details.${RESET}\n"

read -rp "Type YES to continue: " confirm
if [[ "${confirm}" != "YES" ]]; then
    info "Aborted by user."
    exit 0
fi

# ---------------------------------------------------------------------------
# Step 6: Start daemon
# ---------------------------------------------------------------------------
step "Step 6 — Start pcloudcc daemon"

KAT_TIMESTAMP="$(date +%Y%m%d-%H%M%S)"
KAT_FOLDER_NAME="${KAT_FOLDER_PREFIX}-${KAT_TIMESTAMP}"

cat <<MSG

  Run this command in a SEPARATE terminal and wait until you see
  "pCloud account logged in." or similar:

    ${PCLOUDCC} --username ${PCLOUD_USERNAME} --password --daemonize

  When prompted for password, enter your pCloud LOGIN password (from .env).
  This is your account password, NOT the crypto password.

MSG
pause

# ---------------------------------------------------------------------------
# Step 7: Crypto setup
# ---------------------------------------------------------------------------
step "Step 7 — Crypto setup via pcloudcc --commands_only"

cat <<MSG

  Run this in a SEPARATE terminal:

    ${PCLOUDCC} --commands_only

  At the pcloudcc prompt, type:

    crypto setup

  When asked for the crypto password, enter EXACTLY:

    pclsync-kat-fixture-v1-do-not-use-for-real

  Confirm it when prompted again.  Then type 'exit' or Ctrl-D.

MSG
pause

# ---------------------------------------------------------------------------
# Step 8: Create KAT folder
# ---------------------------------------------------------------------------
step "Step 8 — Create crypto folder ${KAT_FOLDER_NAME}"

cat <<MSG

  In ${PCLOUDCC} --commands_only, run:

    mkdir /${KAT_FOLDER_NAME}
    crypto folder /${KAT_FOLDER_NAME}

  This marks the folder as a crypto folder.  Then 'exit'.

MSG
pause

# ---------------------------------------------------------------------------
# Step 9: Upload plaintext files via pcloudcc
# ---------------------------------------------------------------------------
step "Step 9 — Upload known-plaintext files"

cat <<MSG

  In ${PCLOUDCC} --commands_only, run:

    put ${PT_4K} /${KAT_FOLDER_NAME}/kat_4096.bin
    put ${PT_5K} /${KAT_FOLDER_NAME}/kat_5000.bin

  Wait until each upload completes, then 'exit'.

MSG
pause

# ---------------------------------------------------------------------------
# Step 10: Extract ciphertext from pCloud via API
# ---------------------------------------------------------------------------
step "Step 10 — Extract raw ciphertext via pCloud API"

info "Fetching auth token from local pcloudcc state..."

# pcloudcc stores the auth token in its local SQLite DB
if [[ ! -f "${PCLSYNC_DB}" ]]; then
    warn "pcloudcc DB not found at ${PCLSYNC_DB} — trying common alternate paths..."
    PCLSYNC_DB="${HOME}/.pcloud/pcloud.db"
fi

AUTH_TOKEN=""
if [[ -f "${PCLSYNC_DB}" ]]; then
    AUTH_TOKEN=$(sqlite3 "${PCLSYNC_DB}" \
        "SELECT value FROM settings WHERE key='auth' LIMIT 1;" 2>/dev/null || true)
fi

if [[ -z "${AUTH_TOKEN}" ]]; then
    warn "Could not auto-extract auth token from DB."
    read -rp "Paste your pCloud auth token (from pcloudcc --commands_only → 'token'): " AUTH_TOKEN
fi
ok "Auth token acquired (length=${#AUTH_TOKEN})"

# Resolve the folder ID for the KAT folder
info "Resolving folder ID for /${KAT_FOLDER_NAME}..."
FOLDER_META=$(curl -sf \
    "https://api.pcloud.com/listfolder?auth=${AUTH_TOKEN}&path=/${KAT_FOLDER_NAME}&recursive=1" \
    || fatal "listfolder API call failed")

KAT_FOLDER_ID=$(echo "${FOLDER_META}" | python3 -c \
    "import sys,json; d=json.load(sys.stdin); print(d['metadata']['folderid'])" 2>/dev/null || true)

if [[ -z "${KAT_FOLDER_ID}" ]]; then
    fatal "Could not parse folder ID.  Raw response:\n${FOLDER_META}"
fi
ok "KAT folder ID: ${KAT_FOLDER_ID}"

# Extract file IDs
FILE_4K_ID=$(echo "${FOLDER_META}" | python3 -c \
    "import sys,json; d=json.load(sys.stdin)
contents = d['metadata']['contents']
for f in contents:
    if f.get('name','').startswith('kat_4096'):
        print(f['fileid'])
        break" 2>/dev/null || true)

FILE_5K_ID=$(echo "${FOLDER_META}" | python3 -c \
    "import sys,json; d=json.load(sys.stdin)
contents = d['metadata']['contents']
for f in contents:
    if f.get('name','').startswith('kat_5000'):
        print(f['fileid'])
        break" 2>/dev/null || true)

ok "kat_4096 file ID: ${FILE_4K_ID}"
ok "kat_5000 file ID: ${FILE_5K_ID}"

# Get download links
DL_4K=$(curl -sf \
    "https://api.pcloud.com/getfilelink?auth=${AUTH_TOKEN}&fileid=${FILE_4K_ID}" \
    | python3 -c "import sys,json; d=json.load(sys.stdin); \
        hosts=d['hosts']; path=d['path']; print('https://'+hosts[0]+path)" 2>/dev/null || true)

DL_5K=$(curl -sf \
    "https://api.pcloud.com/getfilelink?auth=${AUTH_TOKEN}&fileid=${FILE_5K_ID}" \
    | python3 -c "import sys,json; d=json.load(sys.stdin); \
        hosts=d['hosts']; path=d['path']; print('https://'+hosts[0]+path)" 2>/dev/null || true)

CT_4K="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sector_id_0.ct"
CT_5K="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sector_id_1.ct"

info "Downloading ciphertext for kat_4096..."
curl -sf -o "${CT_4K}" "${DL_4K}" || fatal "Failed to download kat_4096 ciphertext"
ok "Ciphertext written: ${CT_4K} ($(wc -c < "${CT_4K}") bytes)"

info "Downloading ciphertext for kat_5000..."
curl -sf -o "${CT_5K}" "${DL_5K}" || fatal "Failed to download kat_5000 ciphertext"
ok "Ciphertext written: ${CT_5K} ($(wc -c < "${CT_5K}") bytes)"

# ---------------------------------------------------------------------------
# Step 11: Extract auth tags (trailing 32 bytes of each sector)
# ---------------------------------------------------------------------------
step "Step 11 — Extract auth tags from ciphertext"

# pclsync appends a 32-byte HMAC-SHA256 auth tag at the end of each sector blob
TAG_0="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sector_id_0_auth_tag.bin"
TAG_1="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sector_id_1_auth_tag.bin"

ct0_size=$(wc -c < "${CT_4K}")
ct1_size=$(wc -c < "${CT_5K}")

python3 - <<PYEOF
with open("${CT_4K}", "rb") as f:
    data = f.read()
with open("${TAG_0}", "wb") as f:
    f.write(data[-32:])
print(f"Auth tag 0 extracted ({len(data[-32:])} bytes)")
PYEOF

python3 - <<PYEOF
with open("${CT_5K}", "rb") as f:
    data = f.read()
with open("${TAG_1}", "wb") as f:
    f.write(data[-32:])
print(f"Auth tag 1 extracted ({len(data[-32:])} bytes)")
PYEOF

ok "Auth tags written: ${TAG_0}, ${TAG_1}"

# ---------------------------------------------------------------------------
# Step 12: Extract master auth (root hash from the 2-sector file)
# ---------------------------------------------------------------------------
step "Step 12 — Extract master auth tag for 2-sector file"

MASTER_AUTH="${REPO_ROOT}/${FIXTURE_DIR}/c_client_master_auth.bin"

# The master auth tag in pclsync is stored at the head of the file blob
# (first 32 bytes of the .ct file for the multi-sector case).
python3 - <<PYEOF
with open("${CT_5K}", "rb") as f:
    data = f.read()
with open("${MASTER_AUTH}", "wb") as f:
    f.write(data[:32])
print(f"Master auth written ({len(data[:32])} bytes from start of 5000-byte ct)")
PYEOF

ok "Master auth written: ${MASTER_AUTH}"

# ---------------------------------------------------------------------------
# Step 13: Extract wrapped private key and salt from API
# ---------------------------------------------------------------------------
step "Step 13 — Extract wrapped private key and PBKDF2 salt"

USERKEYS_RESPONSE=$(curl -sf \
    "https://api.pcloud.com/crypto_getuserkeys?auth=${AUTH_TOKEN}" \
    || fatal "crypto_getuserkeys API call failed")

PRIV_KEY_WRAPPED_B64=$(echo "${USERKEYS_RESPONSE}" | python3 -c \
    "import sys,json,base64; d=json.load(sys.stdin); \
     print(d.get('privatekey','') or d.get('private_key',''))" 2>/dev/null || true)

PRIV_KEY_SALT_B64=$(echo "${USERKEYS_RESPONSE}" | python3 -c \
    "import sys,json,base64; d=json.load(sys.stdin); \
     print(d.get('privatekey_salt','') or d.get('salt','') or d.get('password_salt',''))" 2>/dev/null || true)

if [[ -z "${PRIV_KEY_WRAPPED_B64}" ]]; then
    warn "Could not auto-parse privatekey from API response. Dumping raw response:"
    echo "${USERKEYS_RESPONSE}"
    warn "Manually set PRIV_KEY_WRAPPED_B64 and PRIV_KEY_SALT_B64 and re-run from Step 13."
fi

PRIV_KEY_WRAPPED="${REPO_ROOT}/${FIXTURE_DIR}/c_client_priv_key_wrapped.bin"
PRIV_KEY_SALT="${REPO_ROOT}/${FIXTURE_DIR}/c_client_priv_key_salt.bin"

python3 - <<PYEOF
import base64, sys
b64 = """${PRIV_KEY_WRAPPED_B64}"""
data = base64.b64decode(b64.strip())
with open("${PRIV_KEY_WRAPPED}", "wb") as f:
    f.write(data)
print(f"Wrapped priv key written: {len(data)} bytes")
PYEOF

python3 - <<PYEOF
import base64, sys
b64 = """${PRIV_KEY_SALT_B64}"""
if not b64.strip():
    # salt may be hex-encoded in some API versions
    print("WARNING: salt field empty — writing zero-length salt file")
    data = b""
else:
    data = base64.b64decode(b64.strip())
with open("${PRIV_KEY_SALT}", "wb") as f:
    f.write(data)
print(f"Priv key salt written: {len(data)} bytes")
PYEOF

ok "Wrapped priv key: ${PRIV_KEY_WRAPPED}"
ok "Priv key salt:    ${PRIV_KEY_SALT}"

# ---------------------------------------------------------------------------
# Step 14: Extract wrapped symmetric (folder) key
# ---------------------------------------------------------------------------
step "Step 14 — Extract folder symmetric key (RSA-OAEP-wrapped)"

FOLDER_KEYS_RESPONSE=$(curl -sf \
    "https://api.pcloud.com/crypto_getfolderkeys?auth=${AUTH_TOKEN}&folderid=${KAT_FOLDER_ID}" \
    || fatal "crypto_getfolderkeys API call failed")

SYM_KEY_B64=$(echo "${FOLDER_KEYS_RESPONSE}" | python3 -c \
    "import sys,json; d=json.load(sys.stdin)
keys = d.get('keys', [d]) if isinstance(d.get('keys'), list) else [d]
for k in keys:
    v = k.get('encryptedkey') or k.get('key') or k.get('symkey') or ''
    if v:
        print(v)
        break" 2>/dev/null || true)

SYM_KEY_WRAPPED="${REPO_ROOT}/${FIXTURE_DIR}/c_client_sym_key_wrapped.bin"

python3 - <<PYEOF
import base64, sys
b64 = """${SYM_KEY_B64}"""
if not b64.strip():
    print("WARNING: sym key field empty — check API field names in: ${FOLDER_KEYS_RESPONSE}")
    sys.exit(0)
data = base64.b64decode(b64.strip())
with open("${SYM_KEY_WRAPPED}", "wb") as f:
    f.write(data)
print(f"Wrapped sym key written: {len(data)} bytes")
PYEOF

ok "Wrapped sym key: ${SYM_KEY_WRAPPED}"

# ---------------------------------------------------------------------------
# Step 15: Extract filename-encryption key material
# ---------------------------------------------------------------------------
step "Step 15 — Extract AES and HMAC keys for filename encryption"

# pclsync derives a 32-byte AES key and 32-byte HMAC-SHA256 key from the
# folder sym key for filename encryption.  These are embedded in the first
# 64 bytes of the decrypted sym_key_ver1 blob.
# We cannot decrypt here (that requires the private key + KAT password).
# Instead we record a placeholder that Wave 1 primitives will fill during
# the decryption KAT round-trip test.

AES_KEY_FILE="${REPO_ROOT}/${FIXTURE_DIR}/c_client_filename_hello_txt_aes_key.bin"
HMAC_KEY_FILE="${REPO_ROOT}/${FIXTURE_DIR}/c_client_filename_hello_txt_hmac_key.bin"
FILENAME_ENC="${REPO_ROOT}/${FIXTURE_DIR}/c_client_filename_hello_txt.b32"

cat <<'MSG'

  STEP 15 NOTE:
  The AES key and HMAC key for filename encryption are derived from the
  decrypted folder sym key.  They cannot be extracted here without
  decrypting the RSA-OAEP-wrapped sym key, which requires the private key
  decrypted with KAT_CRYPTO_PASSWORD.

  Placeholder files will be written.  Wave 1 KAT tests that cover filename
  encryption must:
    1. Decrypt priv key with KAT_CRYPTO_PASSWORD + salt (PBKDF2-SHA512, 20000 iter).
    2. RSA-OAEP-decrypt the folder sym key.
    3. Split bytes [0:32] → AES key, [32:64] → HMAC key.
    4. Overwrite these placeholder files and commit.

MSG

# Write zero-filled placeholders so Wave 1 knows where to put real data
python3 -c "open('${AES_KEY_FILE}', 'wb').write(b'PLACEHOLDER-32B-AES-KEY-FILL-ME!!')"
python3 -c "open('${HMAC_KEY_FILE}', 'wb').write(b'PLACEHOLDER-32B-HMAC-KEY-FILL-ME!')"
python3 -c "open('${FILENAME_ENC}', 'w').write('PLACEHOLDER-BASE32-ENCRYPTED-FILENAME-FOR-hello.txt\n')"

warn "Placeholder key files written.  Must be filled by Wave 1 decrypt pass."

# ---------------------------------------------------------------------------
# Step 16: Write fixture README
# ---------------------------------------------------------------------------
step "Step 16 — Write fixture README.md"

PCLOUDCC_VER="${PCLOUDCC_VERSION:-unknown}"
EXTRACT_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

cat > "${REPO_ROOT}/${FIXTURE_DIR}/README.md" <<HEREDOC
# FIXTURE — NOT FOR PRODUCTION

This directory contains raw binary test fixtures extracted from the official
pCloud C client (pcloudcc) for use in Known-Answer Tests (KAT) of the Rust
pcloud-crypto crate.

## Safety statement

- These fixtures are bound to the public KAT password below.
- No real user secret is embedded here.
- The wrapped private key blob is useless without the KAT password.
- The plaintext content is entirely synthetic (no PII).

## Extraction metadata

| Field              | Value |
|--------------------|-------|
| Extracted on       | ${EXTRACT_DATE} |
| pcloudcc version   | ${PCLOUDCC_VER} |
| pCloud account     | ${PCLOUD_USERNAME} |
| KAT crypto password| pclsync-kat-fixture-v1-do-not-use-for-real |
| KAT folder         | /${KAT_FOLDER_NAME} |
| KAT folder ID      | ${KAT_FOLDER_ID} |

## Re-extraction

Run:

    bash scripts/extract-pclsync-kat.sh

Read scripts/extract-pclsync-kat.md before running.

## Fixture file inventory

| File | Description | Bytes |
|------|-------------|-------|
| c_client_priv_key_wrapped.bin | PBKDF2+AES-CBC encrypted RSA private key blob (from crypto_getuserkeys) | $(wc -c < "${PRIV_KEY_WRAPPED}" 2>/dev/null || echo '?') |
| c_client_priv_key_salt.bin | 64-byte PBKDF2-SHA512 salt for private key encryption | $(wc -c < "${PRIV_KEY_SALT}" 2>/dev/null || echo '?') |
| c_client_sym_key_wrapped.bin | RSA-OAEP-wrapped folder symmetric key (sym_key_ver1) | $(wc -c < "${SYM_KEY_WRAPPED}" 2>/dev/null || echo '?') |
| c_client_sector_id_0.ct | Encrypted sector 0 bytes for kat_4096.bin | $(wc -c < "${CT_4K}" 2>/dev/null || echo '?') |
| c_client_sector_id_0_auth_tag.bin | 32-byte HMAC-SHA256 auth tag for sector 0 | $(wc -c < "${TAG_0}" 2>/dev/null || echo '?') |
| c_client_sector_id_0_plaintext.bin | Expected decrypted plaintext for sector 0 (4096 bytes) | $(wc -c < "${PT_4K}" 2>/dev/null || echo '?') |
| c_client_sector_id_1.ct | Encrypted sector 1 bytes for kat_5000.bin | $(wc -c < "${CT_5K}" 2>/dev/null || echo '?') |
| c_client_sector_id_1_auth_tag.bin | 32-byte HMAC-SHA256 auth tag for sector 1 | $(wc -c < "${TAG_1}" 2>/dev/null || echo '?') |
| c_client_sector_id_1_plaintext.bin | Expected decrypted plaintext for sector 1 (5000 bytes) | $(wc -c < "${PT_5K}" 2>/dev/null || echo '?') |
| c_client_master_auth.bin | 32-byte root hash for the 2-sector kat_5000.bin file | $(wc -c < "${MASTER_AUTH}" 2>/dev/null || echo '?') |
| c_client_filename_hello_txt.b32 | Base32-encoded encrypted filename for "hello.txt" (PLACEHOLDER) | — |
| c_client_filename_hello_txt_aes_key.bin | AES-256 key for filename encryption (PLACEHOLDER — fill by Wave 1) | — |
| c_client_filename_hello_txt_hmac_key.bin | HMAC-SHA256 key for filename-enc MAC (PLACEHOLDER — fill by Wave 1) | — |

## PBKDF2 parameters (from pclsync source)

- Algorithm: PBKDF2-SHA512
- Iterations: 20 000
- Output length: 32 bytes (AES-256 key for private key decryption)
- Salt: c_client_priv_key_salt.bin

## KAT test flow (Wave 1)

1. Derive AES key: PBKDF2-SHA512(KAT_CRYPTO_PASSWORD, salt, 20000, 32)
2. Decrypt priv key: AES-256-CBC(wrapped_priv_key, derived_key)
3. RSA-OAEP decrypt: sym_key = RSA_OAEP_decrypt(priv_key, c_client_sym_key_wrapped.bin)
4. Sector decrypt: sector_plaintext = pclsync_sector_decrypt(sector.ct, sym_key, sector_id)
5. Verify: sector_plaintext == c_client_sector_id_0_plaintext.bin
6. Filename decrypt: plaintext_name = pclsync_filename_decrypt(c_client_filename_hello_txt.b32, aes_key, hmac_key)
7. Verify: plaintext_name == "hello.txt"
HEREDOC

ok "Fixture README written: ${REPO_ROOT}/${FIXTURE_DIR}/README.md"

# ---------------------------------------------------------------------------
# Step 17: Summary
# ---------------------------------------------------------------------------
step "Extraction complete"

echo ""
echo -e "${GREEN}Fixture files written to: ${REPO_ROOT}/${FIXTURE_DIR}/${RESET}"
echo ""
ls -lh "${REPO_ROOT}/${FIXTURE_DIR}/"
echo ""
echo -e "${YELLOW}IMPORTANT: Commit these fixture files.  Do NOT commit your .env or pcloudcc binary.${RESET}"
echo -e "${YELLOW}The three PLACEHOLDER files (aes_key, hmac_key, filename.b32) must be filled${RESET}"
echo -e "${YELLOW}by Wave 1 after implementing the private-key decryption path.${RESET}"

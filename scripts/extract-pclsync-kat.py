#!/usr/bin/env python3
"""Extract a real pcloud-C-client KAT fixture for the pclsync-compat
crypto interop test.

Pre-requisite (one-time, MANUAL, via the pCloud web UI):

  1. Crypto is already set up on the account (confirmed — paid feature active).
  2. Open https://my.pcloud.com → Crypto Folder → create a subfolder called
     `pclsync-kat-v1`.
  3. Upload the file `crates/pcloud-crypto/tests/fixtures/pclsync_v2/kat-plaintext-v1.bin`
     (4096 bytes, committed in this repo) into that subfolder using the
     pCloud web UI "upload" button. The web UI will encrypt it client-side
     using pCloud's crypto scheme — that's exactly the ciphertext we want.

Then run this script. It uses the credentials in `.env` (direnv-loaded or
sourced) and the pCloud HTTP API directly; it does NOT touch pcloudcc.

The script writes:

  crates/pcloud-crypto/tests/fixtures/pclsync_v2/
    ├── kat-plaintext-v1.bin          ← committed (known input)
    ├── kat-priv-key-ver1.blob        ← extracted (wrapped priv key)
    ├── kat-pub-key-ver1.blob         ← extracted (pub key)
    ├── kat-folder-sym-key-wrapped.bin ← extracted (RSA-OAEP-wrapped folder key)
    ├── kat-file-sym-key-wrapped.bin  ← extracted (RSA-OAEP-wrapped file key)
    ├── kat-file-hash.txt             ← extracted (pCloud hash field)
    ├── kat-ciphertext-v1.bin         ← extracted (encrypted file bytes)
    └── README.md                     ← generated (provenance + how to
                                        re-run + Rust-test flow)

The KAT password is the account login password. Tests read it from
$PCLOUD_KAT_PASSWORD at runtime; do NOT commit it to git.

This script is safe to run more than once — it overwrites the extracted
artifacts in place.
"""

from __future__ import annotations

import hashlib
import hmac
import json
import os
import pathlib
import sys
import urllib.parse
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIX = ROOT / "crates" / "pcloud-crypto" / "tests" / "fixtures" / "pclsync_v2"
API_HOST = os.environ.get("PCLOUD_API_HOST", "eapi.pcloud.com")
API_BASE = f"https://{API_HOST}"
KAT_FOLDER_NAME = "pclsync-kat-v1"
KAT_FILE_NAME = "kat-plaintext-v1.bin"
CRYPTO_ROOT_NAME = "Crypto Folder"  # default name on pCloud


def die(msg: str, code: int = 1) -> "None":
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(code)


def api(method: str, **params: object) -> dict:
    """Call a pCloud JSON-RPC-style HTTP API method and return the parsed
    JSON response body.  Raises on transport errors; caller must check the
    `result` field for pCloud-server-side errors.
    """
    qs = urllib.parse.urlencode({k: v for k, v in params.items() if v is not None})
    url = f"{API_BASE}/{method}?{qs}"
    req = urllib.request.Request(url, method="GET")
    req.add_header("User-Agent", "pcloud-rs-kat-extractor/1.0")
    with urllib.request.urlopen(req, timeout=30) as resp:  # noqa: S310 (https only)
        body = resp.read()
    try:
        data = json.loads(body)
    except json.JSONDecodeError as exc:
        die(f"non-JSON response from {method}: {exc}; body={body[:200]!r}")
    if not isinstance(data, dict):
        die(f"non-dict response from {method}: {type(data).__name__}")
    return data  # type: ignore[no-any-return]


def require_ok(method: str, data: dict) -> dict:
    result = data.get("result")
    if result != 0:
        die(f"{method} returned result={result} error={data.get('error', '<none>')}")
    return data


def compute_password_digest(username: str, password: str, server_digest: str) -> str:
    """Matches pcloud-proto::auth_api::compute_password_digest — pCloud's
    challenge-response. Plaintext-password auth on `userinfo` is rejected
    with result=2000; the server accepts only the digest variant.
    """
    user_lc = username.lower().encode("utf-8")
    user_hex = hashlib.sha1(user_lc).hexdigest()
    h = hashlib.sha1()
    h.update(password.encode("utf-8"))
    h.update(user_hex.encode("ascii"))
    h.update(server_digest.encode("ascii"))
    return h.hexdigest()


def fetch_token(username: str, password: str) -> str:
    print(f"[*] authenticating as {username}")
    # Step 1: get a server digest challenge.
    dg = require_ok("getdigest", api("getdigest"))
    server_digest = dg["digest"]
    # Try plaintext password first (HTTPS-only, documented on pCloud's
    # public API); if rejected, fall back to digest challenge-response
    # which matches the C client's login flow.
    data = api("login", username=username, password=password, getauth=1)
    if data.get("result") != 0:
        # Digest fallback.
        dg = require_ok("getdigest", api("getdigest"))
        server_digest = dg["digest"]
        pw_digest = compute_password_digest(username, password, server_digest)
        data = require_ok(
            "login",
            api(
                "login",
                username=username,
                digest=server_digest,
                passworddigest=pw_digest,
                timeformat="timestamp",
                osversion="linux",
                appversion="pcloud-rs",
                deviceid="pcloud-rs-kat",
                device="Desktop",
                os=5,
                getauth=1,
            ),
        )
    token = data.get("auth")
    if not isinstance(token, str) or not token:
        die("userinfo did not return an 'auth' token (2FA enabled? locked account?)")
    return token


def find_folder(auth: str, parent_id: int, name: str) -> dict:
    data = require_ok(
        "listfolder", api("listfolder", auth=auth, folderid=parent_id, nofiles=0)
    )
    for entry in data.get("metadata", {}).get("contents", []):
        if entry.get("isfolder") and entry.get("name") == name:
            return entry  # type: ignore[no-any-return]
    die(f"folder {name!r} not found under folderid={parent_id}")
    return {}


def find_file(auth: str, parent_id: int, name: str) -> dict:
    data = require_ok(
        "listfolder", api("listfolder", auth=auth, folderid=parent_id, nofiles=0)
    )
    for entry in data.get("metadata", {}).get("contents", []):
        if (not entry.get("isfolder")) and entry.get("name") == name:
            return entry  # type: ignore[no-any-return]
    die(f"file {name!r} not found under folderid={parent_id}")
    return {}


def decode_maybe_hex_or_base64(value: str, expected_len: int | None = None) -> bytes:
    """pCloud returns RSA-wrapped + raw-key blobs as either hex or base64
    strings depending on the endpoint. The heuristic-order matters: a
    base64 string may coincidentally contain only hex-range chars and
    silently lose information if decoded as hex.

    When `expected_len` is provided, every decoder is attempted and only
    the one whose output length matches is returned. This catches the
    504-vs-512 bug where `filekey["key"]` is base64 that happens to look
    almost-hex.
    """
    import base64
    import binascii

    stripped = "".join(value.split())  # strip whitespace/newlines

    candidates: list[tuple[str, bytes]] = []

    # Try hex (only if length and charset match).
    if all(c in "0123456789abcdefABCDEF" for c in stripped) and len(stripped) % 2 == 0:
        try:
            candidates.append(("hex", binascii.unhexlify(stripped)))
        except binascii.Error:
            pass

    # Try base64 (standard + url-safe, with auto-padding).
    padded = stripped + "=" * (-len(stripped) % 4)
    for name, decoder in (("b64-std", base64.b64decode), ("b64-url", base64.urlsafe_b64decode)):
        try:
            candidates.append((name, decoder(padded)))
        except (binascii.Error, ValueError):
            pass

    if not candidates:
        die(f"blob is neither hex nor base64 (len={len(stripped)}, sample={stripped[:32]!r})")

    if expected_len is not None:
        for name, out in candidates:
            if len(out) == expected_len:
                return out
        shapes = ", ".join(f"{n}={len(o)}B" for n, o in candidates)
        die(
            f"no decoder produced expected length {expected_len}B for blob "
            f"(tried: {shapes}, sample={stripped[:32]!r})"
        )

    # No expected length — return the first successful decoder.
    return candidates[0][1]


def main() -> None:
    username = os.environ.get("PCLOUD_USERNAME") or os.environ.get("PCLOUD_TEST_USER")
    password = os.environ.get("PCLOUD_PASSWORD") or os.environ.get("PCLOUD_TEST_PASSWORD")
    if not username or not password:
        die(
            "PCLOUD_USERNAME / PCLOUD_PASSWORD (or PCLOUD_TEST_USER / PCLOUD_TEST_PASSWORD) "
            "must be set. Source .env or run under direnv."
        )

    plaintext_path = FIX / "kat-plaintext-v1.bin"
    if not plaintext_path.is_file():
        die(f"missing plaintext fixture at {plaintext_path} — checked in?")
    plaintext = plaintext_path.read_bytes()
    expected_sha = hashlib.sha256(plaintext).hexdigest()
    if len(plaintext) != 4096:
        die(f"plaintext fixture is {len(plaintext)} bytes; expected exactly 4096")
    print(f"[*] plaintext sha256 = {expected_sha}")

    auth = fetch_token(username, password)
    print("[*] auth token acquired")

    # --- user keys (wrapped priv + pub) ------------------------------------
    print("[*] fetching crypto_getuserkeys")
    userkeys = require_ok("crypto_getuserkeys", api("crypto_getuserkeys", auth=auth))
    priv_blob_b64 = userkeys["privatekey"]
    pub_blob_b64 = userkeys["publickey"]
    priv_blob = decode_maybe_hex_or_base64(priv_blob_b64)
    pub_blob = decode_maybe_hex_or_base64(pub_blob_b64)
    print(f"    priv_key_ver1 blob: {len(priv_blob)} bytes")
    print(f"    pub_key_ver1 blob : {len(pub_blob)} bytes")

    # --- locate folder + file ---------------------------------------------
    #
    # Inside the Crypto Folder all names are base32-encoded ciphertext; we
    # can't search by plaintext "pclsync-kat-v1". Instead we scan every
    # encrypted subfolder and look for one that contains exactly one
    # 4096-byte child — that's our KAT fixture by construction.
    print("[*] locating crypto folder tree")
    crypto_root = find_folder(auth, 0, CRYPTO_ROOT_NAME)
    crypto_root_id = int(crypto_root["folderid"])
    print(f"    {CRYPTO_ROOT_NAME} folderid={crypto_root_id}")

    root_listing = require_ok(
        "listfolder",
        api("listfolder", auth=auth, folderid=crypto_root_id, nofiles=0),
    )
    candidates: list[tuple[int, dict, int]] = []
    # For a 4096-byte plaintext, pCloud stores the single ciphertext
    # sector (4096 bytes) + the 32-byte detached auth tag inline when
    # needmasterauth=false (file size ≤ PSYNC_CRYPTO_SECTOR_SIZE).  The
    # on-server "size" therefore equals 4128 bytes.
    EXPECTED_SIZE = 4096 + 32
    for child in root_listing["metadata"].get("contents", []):
        if not child.get("isfolder") or not child.get("encrypted"):
            continue
        sub = require_ok(
            "listfolder",
            api("listfolder", auth=auth, folderid=int(child["folderid"]), nofiles=0),
        )
        files_match = [
            f
            for f in sub["metadata"].get("contents", [])
            if (not f.get("isfolder")) and int(f.get("size", -1)) == EXPECTED_SIZE
        ]
        if len(files_match) == 1:
            candidates.append(
                (int(child["folderid"]), files_match[0], len(sub["metadata"].get("contents", [])))
            )

    if not candidates:
        die(
            "no encrypted subfolder of Crypto Folder contains a 4096-byte file — "
            "confirm kat-plaintext-v1.bin was uploaded to the pclsync-kat-v1 folder"
        )
    if len(candidates) > 1:
        ids = ", ".join(str(c[0]) for c in candidates)
        die(
            f"multiple candidate KAT folders found (folderids={ids}); "
            f"set PCLOUD_KAT_FOLDER_ID=<N> and re-run to disambiguate"
        )
    kat_folder_id, kat_file, child_count = candidates[0]
    kat_file_id = int(kat_file["fileid"])
    kat_file_size = int(kat_file["size"])
    print(f"    KAT folder  folderid={kat_folder_id}  (contains {child_count} child(ren))")
    print(f"    KAT file    fileid={kat_file_id}  size={kat_file_size}")

    # --- folder + file sym keys (RSA-OAEP-wrapped) ------------------------
    print("[*] fetching crypto_getfolderkey")
    fkey = require_ok(
        "crypto_getfolderkey", api("crypto_getfolderkey", auth=auth, folderid=kat_folder_id)
    )
    # RSA-OAEP(RSA-4096) output is always exactly 512 bytes.
    folder_wrapped = decode_maybe_hex_or_base64(fkey["key"], expected_len=512)
    print(f"    folder wrapped sym_key: {len(folder_wrapped)} bytes")

    print("[*] fetching crypto_getfilekey")
    filekey = require_ok(
        "crypto_getfilekey", api("crypto_getfilekey", auth=auth, fileid=kat_file_id)
    )
    # RSA-OAEP(RSA-4096) output is always exactly 512 bytes.
    file_wrapped = decode_maybe_hex_or_base64(filekey["key"], expected_len=512)
    file_hash = int(filekey["hash"])
    print(f"    file wrapped sym_key: {len(file_wrapped)} bytes")
    print(f"    file hash (u64): {file_hash}")

    # --- raw ciphertext via getfilelink + direct HTTPS --------------------
    print("[*] fetching getfilelink")
    link = require_ok("getfilelink", api("getfilelink", auth=auth, fileid=kat_file_id))
    hosts = link["hosts"]
    path = link["path"]
    url = f"https://{hosts[0]}{path}"
    print(f"    downloading {url[:80]}...")
    req = urllib.request.Request(url, method="GET")
    req.add_header("User-Agent", "pcloud-rs-kat-extractor/1.0")
    with urllib.request.urlopen(req, timeout=60) as resp:  # noqa: S310 (https only)
        ciphertext = resp.read()
    print(f"    ciphertext: {len(ciphertext)} bytes")

    # --- write fixtures ---------------------------------------------------
    (FIX / "kat-priv-key-ver1.blob").write_bytes(priv_blob)
    (FIX / "kat-pub-key-ver1.blob").write_bytes(pub_blob)
    (FIX / "kat-folder-sym-key-wrapped.bin").write_bytes(folder_wrapped)
    (FIX / "kat-file-sym-key-wrapped.bin").write_bytes(file_wrapped)
    (FIX / "kat-file-hash.txt").write_text(f"{file_hash}\n")
    (FIX / "kat-ciphertext-v1.bin").write_bytes(ciphertext)

    # README with provenance
    import datetime

    iso = datetime.datetime.now(datetime.timezone.utc).isoformat()
    salt_hint = userkeys.get("salt", "")
    iter_hint = userkeys.get("iterations", "")
    readme = (
        "# pclsync-compat KAT fixtures\n\n"
        f"Extracted on {iso} from {API_BASE} using the account owner's login.\n\n"
        "## Files\n\n"
        "- `kat-plaintext-v1.bin` — committed, known input (4096 bytes, byte[i] = i % 256)\n"
        f"  sha256: `{expected_sha}`\n"
        "- `kat-priv-key-ver1.blob` — RSA-4096 priv key wrapped with PBKDF2-HMAC-SHA512(login_password)\n"
        "- `kat-pub-key-ver1.blob` — RSA-4096 public key (unwrapped)\n"
        "- `kat-folder-sym-key-wrapped.bin` — RSA-OAEP-wrapped sym_key_ver1 for the KAT folder\n"
        "- `kat-file-sym-key-wrapped.bin` — RSA-OAEP-wrapped sym_key_ver1 for the KAT file\n"
        "- `kat-file-hash.txt` — pCloud file-hash (u64, decimal)\n"
        "- `kat-ciphertext-v1.bin` — raw ciphertext as served by pCloud's direct-download endpoint\n\n"
        "## Extraction metadata (from crypto_getuserkeys)\n\n"
        f"- iterations hint: `{iter_hint}`\n"
        f"- salt hint (may be empty — salt lives inside priv_key_ver1 blob): `{salt_hint}`\n\n"
        "## Rust test flow\n\n"
        "See `crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs`. Gated on\n"
        "`PCLOUD_KAT_PASSWORD` env var (= the login password used during extraction).\n"
        "Test is skipped unless both the env var and all fixture files are present.\n\n"
        "## Re-running\n\n"
        "`python3 scripts/extract-pclsync-kat.py` overwrites the extracted artefacts\n"
        "in place. The manual web-UI setup (folder + plaintext upload) only needs to\n"
        "happen once; after that, re-runs take a few seconds.\n"
    )
    (FIX / "README.md").write_text(readme)

    print()
    print("[+] all fixtures written to:")
    print(f"    {FIX}")
    print()
    print("To run the Rust live-KAT test:")
    print("    export PCLOUD_KAT_PASSWORD=<your login password>")
    print("    cargo test -p pcloud-crypto --test pclsync_compat_kat_live -- --ignored")


if __name__ == "__main__":
    main()

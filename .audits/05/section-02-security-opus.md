# Audit 05 — Section 2: Security (Opus)

Date: 2026-04-18
Scope: secret discipline, auth vault, IPC hardening, TLS enforcement,
dual-backend crypto surface (`CryptoBackend`, `PclsyncCompatProfile`,
`PclsyncCompatState`), new CLI gates, KAT extraction script.

## Summary

Secret discipline is strong. `SecretString` / `SecretBytes`
(`crates/pcloud-secret/src/secret_string.rs:35`, `secret_bytes.rs:23`)
both derive `ZeroizeOnDrop`, redact `Debug`, use constant-time
`PartialEq` via `subtle`, deliberately omit `Clone` and
`Serialize/Deserialize`, and require an audit-visible `clone_secret()`
path. A compile-fail test enforces no-serde. IPC local transport
creates `0o700` runtime dir + `0o600` socket with `SO_PEERCRED` /
`getpeereid(3)` peer-uid enforcement
(`crates/pcloud-ipc/src/transport.rs:621-641`,
`crates/pcloud-ipc/src/platform/{linux,unix}.rs`). Production TLS is
enforced — `ApiEndpoint::validate` refuses `Plaintext` under
`Environment::Production` (`crates/pcloud-config/src/api.rs:137`). The
dual-backend crypto surface redacts secret-bearing fields and keeps
runtime state out of serde; three issues below are the real findings.

## Findings

### HIGH-2.1 — `PclsyncCompatProfile` derives `Debug` over ciphertext priv-key blob

`crates/pcloud-crypto/src/pclsync_compat_profile.rs:108`
```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PclsyncCompatProfile {
    pub priv_key_ver1_blob: Vec<u8>,  // ciphertext RSA priv key
    pub pub_key_ver1_blob: Vec<u8>,
    pub pub_fingerprint: [u8; 32],
    pub flags: u32,
}
```
The `priv_key_ver1_blob` holds AES-256-CTR-wrapped RSA PKCS#1 DER
key material. Even though ciphertext, exposing it via a derived
`Debug` means any `tracing::debug!(?profile)` or error-chain
`{:?}` dump writes 500+ B of wrapped private-key material to logs.
Combined with a weak password, this is a takeaway-crackable artifact.
The peer `PclsyncCompatState` (same file, line 348) has a hand-written
redacting `Debug` — the profile struct must match. Also,
`pub_fingerprint` (an HMAC keyed by the low 32 B of the derived KEK)
is printable here; while it is not the KEK itself, logging it together
with the ciphertext priv key gives an offline dictionary attacker a
cheap oracle to verify candidate passwords.
**Fix:** replace `#[derive(Debug)]` with a manual `Debug` that prints
lengths only, matching `PclsyncCompatState`; also consider
`#[serde(with="...")]` or wrapping the blob in `SecretBytes` and
implementing explicit persistence codec.

### HIGH-2.2 — `SymKeyVer1` derives `Clone` despite being raw AES+HMAC key material

`crates/pcloud-crypto/src/pclsync_rsa.rs:169`
```
#[derive(Clone, ZeroizeOnDrop)]
pub struct SymKeyVer1 { pub aes_key:[u8;32], pub hmac_key:[u8;128], ... }
```
This violates the project rule "no `Clone` on secret-bearing types; use
audit-visible `clone_secret()`" stated explicitly in
`crates/pcloud-secret/src/lib.rs:26-35`. Every `SymKeyVer1.clone()`
doubles the in-memory exposure window silently; the compile-time
autoderef probe that guards `SecretBytes` does not catch this struct
because it uses raw `[u8;N]` fields instead of the wrapper.
`SymKeyVer1` is cached in per-folder/per-file HashMaps
(`pclsync_compat_profile.rs:304-305`), so there is no shortage of
callers that may be tempted to `.clone()` during lookups.
**Fix:** remove `#[derive(Clone)]`; add an explicit `clone_secret()`
method, or store `aes_key`/`hmac_key` inside `SecretBytes`.

### MEDIUM-2.3 — IPC socket parent dir perms set to `0o700` only when missing

`crates/pcloud-ipc/src/transport.rs:622-627`
```
let parent_missing = !parent.exists();
fs::create_dir_all(parent)?;
if parent_missing {
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
}
```
If the parent directory pre-exists with looser modes (e.g. `0o755`
under `XDG_RUNTIME_DIR` override, or a user who `mkdir`ed
`~/.local/share/pcloud` by hand), the code happily binds a `0600`
socket inside it but never tightens the dir. The socket inode is
owner-only but the path is traversable by other users. Even though
`SO_PEERCRED` ultimately rejects foreign peers, a world-traversable
directory leaks connection attempts via the bind path and simplifies
race-condition attacks on socket replacement during daemon restart.
**Fix:** always `set_permissions(parent, 0o700)` or verify metadata
and refuse to bind if mode is looser than owner-only.

### MEDIUM-2.4 — KAT extraction script accepts plaintext-password login

`scripts/extract-pclsync-kat.py:109-115`
```
# Try plaintext password first (HTTPS-only, documented on pCloud's
# API docs), fall back to digest auth if rejected.
data = api("login", username=username, password=password, getauth=1)
```
The script sends the user's live login password in cleartext over
HTTPS to pCloud's production API (`PCLOUD_PASSWORD` env var). While
the transport is TLS, the comment "Plaintext-password auth on
`userinfo` is rejected" at line 92 contradicts the actual fallback
behavior — the script tries plaintext first. Running this against a
real account leaks the password into the requests library's TLS
layer, possibly into shell history if the user inlines it. The
Rust path itself (`pcloud-proto::auth_api::compute_password_digest`)
uses the challenge-response digest and does NOT accept plaintext.
**Fix:** make the script digest-only (call `getdigest` first, always
`passworddigest`), remove the plaintext-first branch, and document
that `PCLOUD_PASSWORD` must be fed from a secret manager, never
a shell literal.

### LOW-2.5 — `--acknowledge-not-interop` gate is a single boolean with no replay protection

`crates/pcloud-cli/src/app.rs:2902-2945` enforces the gate correctly
for the scripted path, and `crypto_setup_picker.rs:105-125` requires
literal `YES` in the tty path. However, the flag flows through IPC
as a plain bool (`crates/pcloud-ipc/src/methods.rs:1106`) with no
daemon-side re-confirmation and no idempotency key, so a foothold
that can speak to the already-authorized IPC socket (same uid, e.g.
a compromised CLI plugin) can silently flip a fresh profile to the
non-interoperable `Enhanced` backend without any re-auth. Not a
secret-leak issue, but an intent-integrity gap.
**Fix:** require an interactive re-prompt on the daemon side if the
request arrives without an active setup session token, or bind the
acknowledgement to the CLI invocation via a nonce.

### LOW-2.6 — `auth_vault.rs` shim no longer checks mode directly

`crates/pcloud-daemon/src/auth_vault.rs:48` re-exports
`crate::vault::file::{clear_token, load_token, store_token}`. The
hardening described in `CLAUDE.md` (0600 file, 0700 dir, ownership
check) must be verified in `vault/file.rs` — not reviewed here, so
Section 2.6 is a pointer, not a finding. Confirm in next pass.

## Good Posture (no change needed)

- `CryptoShell::active_key_material` is `SecretBytes` and
  `#[serde(skip)]` (`pcloud-crypto/src/keys.rs:86`).
- `PclsyncCompatState` is `#[serde(skip)]` and its `Debug` redacts
  (`pclsync_compat_profile.rs:348-356`, `lib.rs:724`).
- `CryptoBackend` enum itself carries no secrets; `Debug`/`Serialize`
  are safe (`crates/pcloud-crypto/src/lib.rs:157`).
- Production TLS enforced (`pcloud-config/src/api.rs:137`),
  file-history and envelope also reject plaintext in production.
- IPC peer auth solid on Linux/BSD/macOS; Windows SID stub flagged.

## Recommendations priority

1. HIGH-2.1 manual `Debug` on `PclsyncCompatProfile` (trivial).
2. HIGH-2.2 drop `Clone` on `SymKeyVer1`, add `clone_secret()`.
3. MEDIUM-2.3 always tighten runtime-dir mode.
4. MEDIUM-2.4 remove plaintext-password branch from KAT script.
5. LOW-2.5 add daemon-side setup nonce.

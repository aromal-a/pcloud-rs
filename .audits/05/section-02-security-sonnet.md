# Section 2: Security — Audit 05, Sonnet Pass
**Date:** 2026-04-18  
**Scope:** Secret discipline, auth vault, IPC credential checks, TLS enforcement, sensitive-data exposure, new dual-backend crypto (`CryptoBackend`, `PclsyncCompatProfile`, `PclsyncCompatState`), crypto-setup UX gate, `scripts/extract-pclsync-kat.py`

---

## CRITICAL (0)

No critical findings in the code paths examined.

---

## HIGH (2)

### H-1: PBKDF2-20k iteration count in PclsyncCompatProfile is dangerously low

**File:** `crates/pcloud-crypto/src/pclsync_kdf.rs:50`, `crates/pcloud-crypto/src/pclsync_compat_profile.rs:75`

`PCLSYNC_PBKDF2_ITERATIONS = 20_000` is the hardcoded C-compatible wire constant. NIST SP 800-132 (2023) recommends at least 600,000 iterations for PBKDF2-SHA512 for password-wrapping applications. 20k iterations on a modern GPU can be brute-forced at ~10–100 million guesses/second, yielding a practical offline attack against any user with a weak or medium-strength crypto passphrase. This is a pCloud wire-format constraint — the value cannot be changed without breaking server-side unwrapping — but it must be prominently documented as a known limitation and the KAT test vector must not be confused with a security validation of the iteration count. There is no compensating measure (e.g. server-side rate limiting or per-attempt lockout) documented for the Rust client path.

**Remediation:** Document clearly in `pclsync_kdf.rs` and in the crypto section of the deployment guide that 20k iterations is a wire-format legacy constraint, not a security recommendation. If pCloud supports a newer protocol version with higher iteration counts, gate the new profile setup on the stronger KDF and reject `priv_key_ver1` blobs with 20k iterations in any non-legacy compatibility mode. Track as a new bead under `bd-1du`.

---

### H-2: `scripts/extract-pclsync-kat.py` — plaintext password passed as environment variable and transmitted over HTTPS to API

**File:** `scripts/extract-pclsync-kat.py:213–215, 112`

The script reads `PCLOUD_PASSWORD` / `PCLOUD_TEST_PASSWORD` from the environment and passes the raw cleartext password to the `login` API endpoint (line 112: `api("login", username=username, password=password, ...)`). Environment variables are visible in `/proc/<pid>/environ` on Linux to processes running as the same user (and to root). The credential is the same password used to wrap the user's RSA-4096 private key — leaking it permits offline unwrap of any previously extracted `priv_key_ver1` blob. Additionally the `fetch_token` function falls through to digest auth only if plaintext-password auth fails (line 113–114), meaning on some endpoint configurations the plaintext password is transmitted in the first attempt.

**Remediation:** Invert the auth ordering: always attempt digest challenge-response first (which the Rust `pcloud-proto` auth path already does). Warn in the script header that the environment variable holds the crypto master-key passphrase. Add a note to the KAT README that the extracted fixture files (`kat-priv-key-ver1.blob`) are sensitive and must not be committed with a password that was also used on a real pCloud account.

---

## MEDIUM (4)

### M-1: `RedactedString::into_string` yields a raw `String` with no zeroize guarantee at the IPC/daemon handoff

**File:** `crates/pcloud-ipc/src/redacted.rs:54–57`

`into_string()` takes the inner buffer via `std::mem::take` but does NOT zeroize the source. The returned `String` is intended to be immediately wrapped in `SecretString`, but there is no type-system enforcement of that contract. If a call site fails to wrap it — or if the intermediate `String` is temporarily stored (e.g., bound in a `let` before the `SecretString::new(...)` call) — the secret persists unprotected on the heap. The `Drop` on `RedactedString` zeroizes when the struct is dropped, but `into_string` moves the buffer out before drop runs.

**Remediation:** Return a `SecretString` directly from a `into_secret()` method, or accept a closure `fn into_secret<R>(self, f: impl FnOnce(SecretString) -> R) -> R` to eliminate the window. Deprecate `into_string`.

---

### M-2: Auth vault `validate_vault_file` does not verify the parent directory mode on load

**File:** `crates/pcloud-daemon/src/vault/file.rs:215–237`

`validate_vault_file` checks the vault file's own mode (line 230: `metadata.mode() & 0o077 != 0`) but does not check the parent directory mode. If the parent directory (`~/.config/pcloud/`) was created with mode 0o755 (e.g., by an external tool before the daemon first ran), an attacker can observe the token filename and race to read the vault file even though the file itself is 0o600, because the directory listing is world-readable. The `store_token` path correctly sets the parent to 0o700 on creation, but only when `parent_missing` is true — if the directory already exists with a relaxed mode, its permissions are not corrected.

**Remediation:** Add a parent-directory mode check in `validate_vault_file`. In `store_token` unconditionally apply `0o700` to the parent directory after `create_dir_all`, not only when it was newly created. Add a test covering the pre-existing-relaxed-parent scenario.

---

### M-3: `PclsyncCompatProfile` derives `Clone`, `Serialize`, `Deserialize` — the struct contains `priv_key_ver1_blob` (ciphertext private key)

**File:** `crates/pcloud-crypto/src/pclsync_compat_profile.rs:108–126`

`PclsyncCompatProfile` is `#[derive(Clone, Serialize, Deserialize)]`. The `priv_key_ver1_blob` field is ciphertext (not plaintext key material), so this is not an immediate secret leak. However, the struct also contains the `pub_fingerprint` (a 32-byte HMAC using the low 32 bytes of the KEK as the key), which is a non-secret per the doc comment but is derived from secret key material. More critically, the `Clone` derive means any code path that inadvertently clones a `PclsyncCompatProfile` silently doubles the ciphertext blob's lifetime in memory, and `Serialize` means the profile can be written to arbitrary sinks (log, HTTP body, IPC response) without any redaction gate. There is no `Debug` redaction for this struct.

**Remediation:** Implement a `Debug` for `PclsyncCompatProfile` that redacts `priv_key_ver1_blob` (show only byte length). Consider removing `Clone` or replacing it with an explicit `clone_profile()` method to make duplication auditable. Add a compile-fail test for `Serialize` of `PclsyncCompatProfile` to an untrusted sink if appropriate.

---

### M-4: IPC socket bind does not correct pre-existing parent directory permissions

**File:** `crates/pcloud-ipc/src/transport.rs:621–642`

`IpcServer::bind` applies `fs::set_permissions(parent, 0o700)` only when `parent_missing` (line 623–626). If the runtime directory already exists with world-readable permissions (e.g., `0o755`), the socket is created in a world-listable directory. A local attacker can discover the socket name from the directory listing and attempt to connect. The server's `SO_PEERCRED` uid check provides defense-in-depth but does not eliminate the socket enumeration vector. The socket file itself is correctly set to `0o600`.

**Remediation:** Unconditionally apply `0o700` to the parent directory in `IpcServer::bind`, regardless of whether it was newly created. Add a test that verifies a pre-existing `0o755` parent is tightened on bind.

---

## LOW (3)

### L-1: `current_effective_uid()` marked unsafe but has trivial safety comment

**File:** `crates/pcloud-ipc/src/auth.rs:65–68`

The `unsafe` block for `libc::geteuid()` has a `// SAFETY:` comment (`geteuid has no preconditions`) which is correct and sufficient. However, `geteuid` can be called safely in Rust without `unsafe` via the `rustix` or `nix` crates. The current use of raw `libc` creates a small `unsafe` surface for a zero-risk call. Not a security issue, but an unnecessary `unsafe` block.

**Remediation:** Consider switching to `rustix::process::getuid()` or `nix::unistd::Uid::effective()` to eliminate the unsafe block.

---

### L-2: `extract-pclsync-kat.py` writes `kat-priv-key-ver1.blob` to the fixtures directory without checking git-ignore

**File:** `scripts/extract-pclsync-kat.py:331`

The script writes extracted server-side blobs to `crates/pcloud-crypto/tests/fixtures/pclsync_v2/`. If a developer accidentally runs `git add .` after extraction and the fixture directory is not in `.gitignore`, the real account's wrapped private key blob would be committed. The blob is ciphertext (not cleartext), but combined with the `PCLOUD_KAT_PASSWORD` env var it allows full key recovery.

**Remediation:** Verify that `crates/pcloud-crypto/tests/fixtures/pclsync_v2/kat-*.blob` and `kat-*.bin` (the extracted files, not the committed plaintext) are listed in `.gitignore`. Print a post-run reminder in the script to confirm that only `kat-plaintext-v1.bin` and `README.md` should be committed.

---

### L-3: `CryptoPolicy::auto_lock_idle_secs = 0` disables auto-lock by default

**File:** `crates/pcloud-crypto/src/policy.rs:62`

The default `CryptoPolicy` has `auto_lock_idle_secs: 0`, which disables the auto-lock timer. On a laptop or shared machine, a user who authenticates crypto and then walks away leaves the master key resident indefinitely. The `lock_on_suspend` default of `true` provides partial mitigation on platforms that signal suspend, but process compromise between wakeup and the next suspend goes unmitigated.

**Remediation:** Consider defaulting `auto_lock_idle_secs` to a non-zero value (e.g., 3600 seconds = 1 hour) in the `Production` environment profile and only defaulting to 0 in `Development`. Document the recommendation in the deployment guide.

---

## Summary

| Severity | Count | Key findings |
|----------|-------|-------------|
| CRITICAL | 0     | — |
| HIGH     | 2     | Weak PBKDF2 iteration count (wire-locked at 20k); plaintext password in KAT extractor script |
| MEDIUM   | 4     | `RedactedString::into_string` lacks zeroize-on-handoff; vault parent-dir mode not corrected on load; `PclsyncCompatProfile` missing `Debug` redaction and exposed `Clone`/`Serialize`; IPC bind doesn't tighten pre-existing parent dir |
| LOW      | 3     | Unnecessary `unsafe` for `geteuid`; KAT fixture gitignore gap; auto-lock defaults to disabled |

**Strengths confirmed by this audit:**
- `SecretString` and `SecretBytes` wrappers are solid: `ZeroizeOnDrop`, constant-time `PartialEq`, redacted `Debug`, no `Serialize`, explicit `clone_secret()`.
- `RedactedString` on the IPC wire fills the serde-boundary gap correctly.
- Auth vault: atomic write (tmp + rename), `0o600` file + `0o700` parent on creation, `symlink_metadata` rejecting non-regular files, UID ownership check, `0o077` mode mask, Windows DPAPI guard.
- IPC: `SO_PEERCRED`/`getpeereid` on every accept, uid match before dispatch, 1 MiB frame cap before allocation, per-peer and global connection caps, 5-second read timeout, 30-second write timeout.
- TLS: `Production` environment rejects `http://` API endpoints at config validation time; `Development` only for local fixtures.
- `PclsyncCompatState::Debug` redacts the RSA private key.
- `CryptoPolicy.persist_master_key = false` is a hard default enforced at every setup/start entry point.
- `unlock_profile` performs the fingerprint constant-time check before parsing the RSA private key, avoiding oracle timing against the wrapped key.
- KAT test vector in `pclsync_kdf.rs` is cross-validated against Python's independent `hashlib.pbkdf2_hmac` output.

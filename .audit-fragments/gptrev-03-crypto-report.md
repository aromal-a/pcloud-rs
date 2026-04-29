# GPTREV/03 Crypto Subsystem — Fix Report

**Date:** 2026-04-26  
**Scope:** `crates/pcloud-crypto/src/`, `crates/pcloud-kms/src/`  
**KAT status:** `kat_c_client_vector` PASSES (PclsyncCompat wire format unchanged)  
**Test result:** 174 lib tests passed, 0 failed; 9 KMS tests passed, 0 failed

---

## Triage Summary

| Finding | Disposition | Action |
|---------|-------------|--------|
| C-01 (password rotation local-before-server) | Partially defer — daemon-level ordering is out of scope; crypto-layer fix is limited to correct KMS staging already present in Enhanced path | Documented; PclsyncCompat path notes same ordering caveat |
| H-01 (KMS not wired in daemon) | Defer — `pcloud-daemon/` is out of scope | No code change |
| H-02 (legacy setup defaults Enhanced) | Defer — daemon dispatch out of scope | No code change |
| H-03 (setup/mkdir local-first) | Defer — server coordination is daemon-level | No code change |
| **H-04 (change_password_unlocked broken for PclsyncCompat)** | **BUG FIXED** | Added backend check at top of `change_password_unlocked`; returns `NotYetWired` for PclsyncCompat instead of silently reading wrong key state |
| H-05 (file-key cache ignores hash) | Annotate/defer — TODO already present | Existing TODO(bd-1du.10) retained |
| H-06 (crypto share not daemon-wired) | Defer — daemon/CLI out of scope | No code change |
| **H-07 (TTL bypassed by mkdir and temppass)** | **BUG FIXED (partial)** | Enhanced `mkdir` now routes through `require_active_key()` instead of reading `active_key_material` directly. `derive_temppass_wire` adds a non-mutating `is_key_stale()` check (eviction deferred to next `require_active_key` call since temppass takes `&CryptoShell`). New `KeyManager::is_key_stale()` method added. |
| **H-08 (Enhanced rotation skips Unicode normalization)** | **BUG FIXED** | `change_password` Enhanced path now normalizes old+new passwords to NFC before derive/compare. `change_password_unlocked` now normalizes `new_password` before derivation. PasswordUnchanged comparison moved after normalization. |
| **M-01 (Vault accepts http://)** | **BUG FIXED** | `HashicorpVault::new` rejects `http://` URLs at construction; `allow-insecure-http` Cargo feature allows override for dev/CI. Added to `Cargo.toml`. |
| M-02 (KMS cache unbounded) | Defer — LRU refactor is a significant new feature | No code change |
| M-03 (dev transport gaps) | Defer — `pcloud-backends/` out of scope | No code change |
| M-04 (proto DTOs Debug) | Defer — `pcloud-proto/` out of scope | No code change |
| **M-05 (metadata NUL not rejected)** | **BUG FIXED** | `encrypt_filename` now rejects filenames containing `\0` alongside the existing empty/`/` checks |
| L-01 (docs overstate status) | Annotate only | Existing `CLAUDE.md` / parity matrix not altered; finding noted here |

---

## Files Modified

- `crates/pcloud-crypto/src/keys.rs` — Added `KeyManager::is_key_stale()` non-mutating TTL check
- `crates/pcloud-crypto/src/lib.rs` — H-04: backend guard in `change_password_unlocked`; H-07: `mkdir` through `require_active_key()`; H-08: NFC normalization in `change_password` and `change_password_unlocked`
- `crates/pcloud-crypto/src/share_temppass.rs` — H-07: `is_key_stale()` check before master key borrow
- `crates/pcloud-crypto/src/metadata.rs` — M-05: NUL byte rejection in `encrypt_filename`
- `crates/pcloud-kms/src/lib.rs` — M-01: HTTP URL rejection in `HashicorpVault::new`
- `crates/pcloud-kms/Cargo.toml` — M-01: `allow-insecure-http` feature declaration

---

## Deferred (with rationale)

- **C-01 ordering**: The `change_password_pclsync_compat_reencoded` body commits local state at line ~2347 before the daemon uploads to the server. Full remediation requires the daemon to stage blobs, attempt upload, then commit — this is `pcloud-daemon/` work tracked under `bd-1du.10`. The Enhanced path already implements staging via `rewrap_single_kms_blob` before local commit.
- **H-01 / H-02 / H-03 / H-06**: All require daemon wiring (`pcloud-daemon/src/`). Out of allowed scope.
- **H-05**: The `unwrap_and_cache_file_key` hash parameter is accepted but discarded with a clear `TODO(bd-1du.10)`. Implementing `(file_id, hash)` keying requires changing the `PclsyncCompatState` cache type and all callers — tracked upstream.
- **M-02 (unbounded DEK cache)**: A bounded LRU would be a new feature requiring `lru` crate or equivalent. Deferred as non-trivial refactor.
- **M-03 / M-04**: Out of scope crates.

---

## Wire-Compat Assurance

- `PclsyncCompat` backend: PBKDF2-HMAC-SHA512 (20 000 iterations), RSA-4096-OAEP, AES-256-CTR sector cipher — **untouched**.
- `kat_c_client_vector` test passes after all edits.
- NFC normalization was already present in `setup_pclsync_compat` (line 1551) and `start_pclsync_compat` (line 1814); no new normalization was added to the PclsyncCompat paths.
- The `change_password_pclsync_compat_reencoded` path already normalized at lines 2280–2281 before this diff.

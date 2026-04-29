# Crypto Subsystem Enterprise Readiness Audit - Subagent 03

Scope: `crates/pcloud-crypto`, `crates/pcloud-kms`, crypto daemon/backend/proto/SDK integration, crypto tests/fuzz/benches, share temppass/RSA share, password rotation, filename metadata, AES-GCM/KDF/nonce handling, zeroization, constant-time checks, C-client KATs, and FIPS/KMS posture. I did not modify files or write `AUDIT_REPORT.md`.

## Verdict

Not enterprise-ready for production crypto interoperability yet. The core Rust crypto primitives are generally disciplined (`forbid(unsafe_code)`, zeroizing wrappers, constant-time comparisons, AES-GCM nonce budget, C-compatible pclsync primitives), but the enterprise readiness blockers are in orchestration: KMS config is not wired into the daemon, PclsyncCompat setup/mkdir/share/password-rotation flows are incomplete or non-transactional, and several key-cache/TTL/Unicode edge cases can cause desync or lockout.

## Findings

1. **CRITICAL C-01 - Password rotation commits local crypto state before durable server success.**  
   Evidence: Enhanced rotation mutates local salt/fingerprint/key before returning at `crates/pcloud-crypto/src/lib.rs:2045`; PclsyncCompat rotation mutates `priv_key_ver1_blob`, fingerprint, and flags at `crates/pcloud-crypto/src/lib.rs:2346`. The daemon uploads only afterward via `crypto_changeuserprivate` at `crates/pcloud-daemon/src/runtime.rs:4480`, and upload failure only returns an error at `crates/pcloud-daemon/src/runtime.rs:4495`. PclsyncCompat docs are internally inconsistent about rotation using `crypto_changeuserkeys`/`crypto_setuserkeys` versus `crypto_changeuserprivate` at `crates/pcloud-crypto/src/lib.rs:2174` and `crates/pcloud-proto/src/methods/crypto.rs:176`.  
   Impact: a network/server/OTP failure can leave the local profile requiring the new password while the server still has old key material, causing cross-device crypto desync or effective lockout.  
   Remediation: stage rekeyed blobs without mutating shell state, upload through one proven C-compatible endpoint, then atomically commit local state only after server success; add rollback and live rotation tests.

2. **HIGH H-01 - `[crypto] mode = "kms"` is declarative only in the daemon.**  
   Evidence: config says the daemon injects a provider before `start` at `crates/pcloud-config/src/crypto_kms.rs:3`; validation accepts non-null KMS providers at `crates/pcloud-config/src/crypto_kms.rs:67`; `CryptoShell` exposes provider injection at `crates/pcloud-crypto/src/lib.rs:1145` and `enable_kms_mode` at `crates/pcloud-crypto/src/lib.rs:1243`. Daemon bootstrap still constructs `CryptoShell::default()` at `crates/pcloud-daemon/src/bootstrap.rs:760`, and `rg` found no daemon `set_kms_provider` / `build_provider` / `enable_kms_mode` call.  
   Impact: operators can configure KMS and believe DEKs are KMS/HSM-backed while the daemon uses the raw local path.  
   Remediation: enable the `pcloud-config/kms-factory` feature in daemon builds, instantiate configured providers, inject them into `CryptoShell`, call `enable_kms_mode`, and fail startup if configured KMS cannot be wired.

3. **HIGH H-02 - Legacy `CryptoSetup` / fresh `CryptoUnlock` silently create Enhanced, non-interoperable profiles.**  
   Evidence: `CryptoBackend::default()` is PclsyncCompat at `crates/pcloud-crypto/src/lib.rs:200`, but legacy `CryptoShell::setup` hardcodes Enhanced at `crates/pcloud-crypto/src/lib.rs:1447`. Daemon `CryptoSetup` uses that legacy method at `crates/pcloud-daemon/src/runtime.rs:3804`; fresh `CryptoUnlock` calls `unlock()`, which performs setup+start at `crates/pcloud-crypto/src/lib.rs:1915`. Only `CryptoSetupV2` enforces the Enhanced acknowledgement at `crates/pcloud-daemon/src/runtime.rs:3924`.  
   Impact: older CLI/IPC flows can create profiles that official pCloud clients cannot decrypt, without explicit acknowledgement or remote key upload.  
   Remediation: reject legacy `CryptoSetup` for new profiles, or route it to `CryptoSetupV2` with PclsyncCompat default; require `acknowledge_not_interop` for every Enhanced creation path.

4. **HIGH H-03 - PclsyncCompat setup and mkdir are local-first and not server-complete.**  
   Evidence: setup commits local PclsyncCompat state before calling `crypto_setuserkeys` at `crates/pcloud-daemon/src/runtime.rs:3998`; comments explicitly leave state committed on server error at `crates/pcloud-daemon/src/runtime.rs:3966`. `CryptoMkdir` creates/caches local state, but server-side wrap/upload is marked out of scope at `crates/pcloud-daemon/src/runtime.rs:3860`. The auto-fetch wrapper exists at `crates/pcloud-daemon/src/runtime.rs:4186` but `crypto_mkdir` calls `mkdir_with_context` directly at `crates/pcloud-daemon/src/runtime.rs:3841`.  
   Impact: the daemon can report successful local crypto folder work that is not remotely created or recoverable by other clients.  
   Remediation: make setup/mkdir transactional against the server, implement the crypto folder create/upload RPC, and use the auto-fetch path in the live dispatch handler.

5. **HIGH H-04 - `CryptoChangePasswordUnlocked` is broken for default PclsyncCompat.**  
   Evidence: PclsyncCompat unlock stores RSA state in `pclsync_compat_state` at `crates/pcloud-crypto/src/lib.rs:1817`; `change_password_unlocked` requires Enhanced `active_key_material` at `crates/pcloud-crypto/src/lib.rs:1980`. Daemon routes the unlocked request directly to that method at `crates/pcloud-daemon/src/runtime.rs:4452`.  
   Impact: the retained C-compatible unlocked password-change flow fails or returns misleading lock errors for the default backend.  
   Remediation: add backend dispatch for unlocked PclsyncCompat rotation or explicitly mark it unsupported until a safe session proof/KEK path exists.

6. **HIGH H-05 - File-key cache ignores server file hash/version.**  
   Evidence: `unwrap_and_cache_file_key` documents that `hash` should be recorded for version validation at `crates/pcloud-crypto/src/lib.rs:2646`, but discards it at `crates/pcloud-crypto/src/lib.rs:2662`. The cache is keyed only by `file_id` at `crates/pcloud-crypto/src/pclsync_compat_profile.rs:317`.  
   Impact: stale file keys can be reused after server-side file version changes, causing failed decrypts or writes under the wrong key.  
   Remediation: key file-cache entries by `(file_id, hash)`, invalidate on hash changes, and require hash in PclsyncCompat sector contexts.

7. **HIGH H-06 - Crypto share/RSA share is not exposed through daemon/CLI orchestration.**  
   Evidence: backend crypto share functions exist at `crates/pcloud-backends/src/shares_backend.rs:484` and RSA variants at `crates/pcloud-backends/src/shares_backend.rs:564`, but daemon dispatch only exposes normal `ShareFolder` and `AccountTeamShare` at `crates/pcloud-daemon/src/runtime.rs:711`. IPC request variants likewise expose only non-crypto share shapes at `crates/pcloud-ipc/src/methods.rs:557`. The temppass helper rejects PclsyncCompat at `crates/pcloud-crypto/src/share_temppass.rs:343`.  
   Impact: C-compatible encrypted sharing/team sharing is backend-only plumbing, not a usable product path.  
   Remediation: add daemon/IPC/CLI/SDK crypto-share requests that fetch recipient/team public keys, fetch/cache folder keys, call the RSA backend path, and exercise a two-account live E2E.

8. **HIGH H-07 - Crypto key TTL is bypassed by folder-name and temppass paths.**  
   Evidence: `require_active_key` is documented as the TTL choke point at `crates/pcloud-crypto/src/lib.rs:1368`; sector derivation uses it at `crates/pcloud-crypto/src/lib.rs:2958`. Enhanced `mkdir` directly reads `active_key_material` at `crates/pcloud-crypto/src/lib.rs:2795`, and `derive_temppass_wire` directly reads it at `crates/pcloud-crypto/src/share_temppass.rs:324`.  
   Impact: expired master key material can still be used for encrypted folder creation or temppass wrapping after the configured TTL.  
   Remediation: centralize key borrowing behind a `&mut self` TTL-enforcing API and make mkdir/temppass paths call it; add TTL expiry tests for both.

9. **HIGH H-08 - Enhanced password rotation skips Unicode normalization.**  
   Evidence: setup/start normalize passwords to NFC at `crates/pcloud-crypto/src/lib.rs:1513` and `crates/pcloud-crypto/src/lib.rs:1721`. Enhanced `change_password` derives the old password raw at `crates/pcloud-crypto/src/lib.rs:2220`, and `change_password_unlocked` derives the new password raw at `crates/pcloud-crypto/src/lib.rs:1993`. PclsyncCompat rotation does normalize at `crates/pcloud-crypto/src/lib.rs:2280`.  
   Impact: visually identical NFC/NFD passwords can fail rotation or rotate to a key that later unlock cannot reproduce.  
   Remediation: normalize old and new passwords in all Enhanced rotation paths before comparisons and derivation; add NFC/NFD rotation tests.

10. **MEDIUM M-01 - Vault KMS accepts plaintext HTTP URLs while sending `X-Vault-Token`.**  
    Evidence: `HashicorpVault::new` stores the provided URL without scheme enforcement at `crates/pcloud-kms/src/lib.rs:571`; requests send `X-Vault-Token` at `crates/pcloud-kms/src/lib.rs:593`.  
    Impact: a misconfigured Vault URL can leak KMS tokens over plaintext HTTP or through a MITM.  
    Remediation: require `https://` by default, allow `http://` only behind an explicit test/dev flag, and document certificate pinning/CA requirements.

11. **MEDIUM M-02 - Global KMS plaintext DEK cache is unbounded and only lazily evicts looked-up entries.**  
    Evidence: `unwrap_cached` stores plaintext DEKs in a process-global cache at `crates/pcloud-kms/src/lib.rs:199`; lookup removes only the queried expired entry at `crates/pcloud-kms/src/lib.rs:244`; store has no size cap at `crates/pcloud-kms/src/lib.rs:255`.  
    Impact: long-lived daemons can retain many plaintext DEKs until exact-key lookup or session stop, increasing memory-residency and DoS risk.  
    Remediation: implement bounded LRU plus global prune/flush hooks, expose metrics, and flush all KMS cache entries on lock/logout.

12. **MEDIUM M-03 - Development crypto transport masks integration gaps.**  
    Evidence: `DevelopmentCryptoTransport` supports only `crypto_sendchangeuserprivate` and `crypto_changeuserprivate` at `crates/pcloud-backends/src/crypto_backend.rs:43`, while runtime methods also expose `set_user_keys`, `get_folder_key`, and `get_file_key` at `crates/pcloud-backends/src/crypto_backend.rs:261`.  
    Impact: daemon tests can pass while PclsyncCompat setup/key-fetch paths fail in development mode or are untested against realistic server responses.  
    Remediation: add dev/mock support for `crypto_setuserkeys`, `crypto_getfolderkey`, `crypto_getfilekey`, and `crypto_getpubkey`, including non-zero server result codes.

13. **MEDIUM M-04 - Sensitive wire DTOs derive `Debug` and clone key blobs/codes as plain strings.**  
    Evidence: `ChangeUserPrivateRequest` derives `Debug` and holds `private_key`, `signature`, and `code` as `String` at `crates/pcloud-proto/src/methods/crypto.rs:36`; params clone those strings at `crates/pcloud-proto/src/methods/crypto.rs:68`. `PclsyncSetUserKeysRequest` similarly derives `Debug` over sealed key blobs at `crates/pcloud-proto/src/methods/crypto.rs:194`.  
    Impact: accidental request logging can expose encrypted private-key blobs, signatures, hints, and confirmation codes.  
    Remediation: implement manual redacted `Debug`, wrap confirmation codes in secret/redacted types, and add tests asserting debug redaction.

14. **MEDIUM M-05 - Enhanced filename metadata is one-way and under-validated.**  
    Evidence: Enhanced filename encoding is HMAC-only at `crates/pcloud-crypto/src/metadata.rs:3`, with no decrypt path; validation rejects only empty names and `/` at `crates/pcloud-crypto/src/metadata.rs:102`. Folder entries persist only encrypted names at `crates/pcloud-crypto/src/lib.rs:2815`.  
    Impact: Enhanced-created folder names cannot be recovered for listings without an external plaintext mapping, and NUL/length edge cases can reach backend/server layers.  
    Remediation: either keep Enhanced explicitly experimental/non-listable, or add a reversible metadata scheme; reject NUL and enforce server/OS byte-length constraints before encoding.

15. **LOW L-01 - Crypto parity documentation overstates implementation status.**  
    Evidence: crate docs say change-password/team crypto are omitted at `crates/pcloud-crypto/src/lib.rs:36`, while parity docs claim password rotation and temppass share implemented at `C_FEATURE_PARITY_REVIEW.md:635`. Matrix rows mark crypto share variants partial/implemented inconsistently at `C_FEATURE_PARITY_MATRIX.csv:124` and `C_FEATURE_PARITY_MATRIX.csv:138`.  
    Impact: operators and reviewers can mistake partial/local-only code for server-compatible enterprise readiness.  
    Remediation: update docs/matrix to distinguish primitive, backend, daemon/CLI, live-E2E, and C-client-compatible completion.

## Positive Controls Observed

- `pcloud-crypto` and `pcloud-kms` both forbid unsafe code: `crates/pcloud-crypto/src/lib.rs:1`, `crates/pcloud-kms/src/lib.rs:30`.
- `SecretString` and `SecretBytes` zeroize on drop, redact `Debug`, avoid implicit `Clone`, and use constant-time equality: `crates/pcloud-secret/src/secret_string.rs:35`, `crates/pcloud-secret/src/secret_bytes.rs:21`.
- Enhanced AES-GCM sector framing binds sector index as AAD and uses random 96-bit nonces: `crates/pcloud-crypto/src/content.rs:179`; shell-level nonce budget uses CAS before sealing at `crates/pcloud-crypto/src/lib.rs:2900`.
- FIPS posture is honest: enabling `crypto-provider-aws-lc-fips` intentionally fails with a compile error, and docs state no validated module ships today at `crates/pcloud-crypto/src/lib.rs:59` and `docs/fips.md:3`.

## Verification

Commands run:

- `sed -n '1,240p' pcloud_rev.md` and `sed -n '241,520p' pcloud_rev.md`
- `rg --files -g '!target/**' -g '!vendor/**' -g '!.beads/**' ...`
- `rg -n "crypto|Crypto|crypt|kms|..." ...`
- `rg -n "CryptoShare|crypto_share|temppass|sharedfolderkey|teamshare_key|..." ...`
- `nl -ba ... | sed -n ...` on cited crypto, KMS, proto, daemon, backend, test, and docs files
- `cargo test -p pcloud-kms`: passed, 9 tests
- `cargo test -p pcloud-crypto`: passed, 174 unit tests plus integration/doc tests; 1 live KAT ignored
- `cargo test -p pcloud-backends crypto_share`: passed, 5 selected tests
- `cargo test -p pcloud-daemon --test crypto_change_password`: passed, 3 tests
- `cargo test -p pcloud-daemon crypto_setup_start_mkdir_cycle_is_active`: passed, 1 selected lib test
- `git status --short --untracked-files=no` and `git diff --stat` for read-only worktree observation

Limitations:

- No live pCloud credentials, no two-account share test, no email OTP automation, and no live AWS/Vault/PKCS#11 provider tests were run.
- Fuzz targets and benches were inspected but not executed.
- Default feature tests were run; `--all-features` was not used because the FIPS seam intentionally triggers `compile_error!`.
- The worktree was already dirty when checked after the audit; I did not apply patches or intentionally edit files.

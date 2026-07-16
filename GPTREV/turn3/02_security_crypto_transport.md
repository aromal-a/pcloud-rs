# Turn 3 Subagent 02: Security, Crypto, Transport Audit

Scope: static read-only audit of security, crypto, auth, secret discipline, and transport surfaces. No files were modified and no `AUDIT_REPORT.md` was written.

## Critical Findings

### C-01 Production config can select Development API mode

**Severity:** Critical

**Evidence:** `crates/pcloud-config/src/api.rs:231` rejects only `Environment::Production` plus `ApiMode::Plaintext`; `ApiMode::Development` is accepted at `crates/pcloud-config/src/api.rs:242`. `PCLOUD_API_MODE=development` is parsed at `crates/pcloud-config/src/env.rs:168`. Auth then switches to `DevelopmentAuthTransport` at `crates/pcloud-backends/src/auth_backend.rs:277`, with fixture credentials and tokens defined at `crates/pcloud-backends/src/auth_backend.rs:39`.

**Impact:** A production daemon can be started against in-process development transports instead of the real TLS transport. That bypasses real authentication, real server policy, and production data-plane behavior.

**Remediation:** Reject `ApiMode::Development` whenever `Environment::Production` is active. Make env overrides fail closed for production plus development/plaintext. Add config tests covering `PCLOUD_ENV=production` with `PCLOUD_API_MODE=development`.

### C-02 HashiCorp Vault KMS accepts cleartext HTTP URLs for token and DEK transport

**Severity:** Critical

**Evidence:** Vault URL validation only checks non-empty strings at `crates/pcloud-config/src/crypto_kms.rs:186`. `HashicorpVault::new` stores the URL without enforcing scheme at `crates/pcloud-kms/src/lib.rs:571`. Vault requests send `X-Vault-Token` at `crates/pcloud-kms/src/lib.rs:595` and send plaintext DEKs in JSON at `crates/pcloud-kms/src/lib.rs:630`.

**Impact:** A config typo or hostile config can send KMS bearer tokens and plaintext data-encryption keys over `http://`, exposing all KMS-protected content.

**Remediation:** Parse Vault URLs with `url::Url`, require `https`, reject hostless URLs, and allow HTTP only behind an explicit test-only feature or environment gate. Add config validation tests for `http://`, missing scheme, and valid `https://`.

## High Findings

### H-01 KMS configuration is validated but not wired into daemon runtime

**Severity:** High

**Evidence:** `crates/pcloud-config/src/crypto_kms.rs:1` documents provider construction and `CryptoShell` injection, but daemon bootstrap initializes `CryptoShell::default()` at `crates/pcloud-daemon/src/bootstrap.rs:742`. Default crypto uses `NullKms` and `CryptoMode::Raw` at `crates/pcloud-crypto/src/lib.rs:1034`. KMS mode only activates through `enable_kms_mode` at `crates/pcloud-crypto/src/lib.rs:1243`, and scoped search found no daemon call to `build_provider`, `set_kms_provider`, or `enable_kms_mode`.

**Impact:** Operators can configure KMS mode and pass validation while the daemon still encrypts locally in raw mode. This is a silent security-control and compliance bypass.

**Remediation:** During daemon bootstrap, build the configured KMS provider, inject it into `CryptoShell`, call `enable_kms_mode`, and fail startup if KMS mode is requested but unavailable. Add an integration test asserting configured KMS mode does not leave provider name as `null`.

### H-02 Encoded protocol requests have unredacted Debug output containing secrets

**Severity:** High

**Evidence:** `BinaryParamValue`, `BinaryParam`, and `EncodedRequest` derive `Debug` at `crates/pcloud-proto/src/binary_api.rs:113`, `crates/pcloud-proto/src/binary_api.rs:148`, and `crates/pcloud-proto/src/binary_api.rs:201`. Auth parameters include `auth`, `token`, `code`, and `passworddigest` in `crates/pcloud-proto/src/methods/auth.rs:198`, `crates/pcloud-proto/src/methods/auth.rs:252`, and `crates/pcloud-proto/src/methods/auth.rs:296`.

**Impact:** Any debug log, trace, test failure, retry diagnostic, or error context involving encoded requests can leak auth tokens, passwords, OTPs, public-link passwords, and raw serialized request bytes.

**Remediation:** Remove derived `Debug` from encoded request and parameter types. Implement custom redacted `Debug` that redacts secret parameter names and reports request bytes only as length or digest. Add regression tests using `format!("{:?}", encoded_request)`.

### H-03 IPC requests leak OTP, recovery, and crypto confirmation codes through Debug

**Severity:** High

**Evidence:** IPC `Request` derives `Debug` at `crates/pcloud-ipc/src/methods.rs:260`. `TwoFactorCodeSubmission.value` is a raw `String` at `crates/pcloud-ipc/src/methods.rs:287`. Crypto password-change confirmation codes are raw `String` fields at `crates/pcloud-ipc/src/methods.rs:333` and `crates/pcloud-ipc/src/methods.rs:351`. A redacting wrapper exists for these values at `crates/pcloud-ipc/src/redacted.rs:32`.

**Impact:** Debugging IPC requests can expose one-time authentication factors and crypto confirmation codes.

**Remediation:** Change these fields to `RedactedString` or an equivalent secret wrapper, then unwrap only at the daemon boundary into `SecretString`. Add Debug redaction tests for all IPC secret-bearing request variants.

### H-04 Auth response DTOs carry bearer tokens as raw debuggable strings

**Severity:** High

**Evidence:** `PasswordLoginOutcome` derives `Debug` at `crates/pcloud-proto/src/auth_api.rs:108`. `Authenticated.auth_token` is a raw `String` at `crates/pcloud-proto/src/auth_api.rs:112`; `TwoFactorRequired.challenge_token` is a raw `String` at `crates/pcloud-proto/src/auth_api.rs:120`. Password change responses also derive `Debug` and carry `auth_token: String` at `crates/pcloud-proto/src/account_api.rs:96`.

**Impact:** Bearer tokens can leak before daemon code wraps them in secret types.

**Remediation:** Parse token-bearing response fields directly into `SecretString` or a redacted protocol token type with zeroizing storage and custom `Debug`.

### H-05 Vault load accepts tokens under insecure parent directories

**Severity:** High

**Evidence:** Vault file validation checks the file owner and mode at `crates/pcloud-daemon/src/vault/file.rs:220`. Parent directory tightening is attempted at `crates/pcloud-daemon/src/vault/file.rs:253`, but if the parent is not owned by the current user the code logs a warning and continues at `crates/pcloud-daemon/src/vault/file.rs:269`. The vault contract requires owner-only parent directories at `crates/pcloud-daemon/src/vault/mod.rs:26`.

**Impact:** Tokens can be loaded from paths under attacker-controlled or shared directories, leaving a symlink/replace race between metadata validation and open.

**Remediation:** Fail closed unless the parent directory is owned by the current user and mode `0700`. Use `openat`/`O_NOFOLLOW`-style opening to bind validation and file open under the trusted directory.

### H-06 Crypto password rotation mutates local state before server commit succeeds

**Severity:** High

**Evidence:** `change_crypto_password` mutates the local crypto shell before upload at `crates/pcloud-daemon/src/runtime.rs:4360`. `CryptoShell::change_password` commits new salt, fingerprint, active key, and KMS state at `crates/pcloud-crypto/src/lib.rs:2045`. The server upload happens later at `crates/pcloud-daemon/src/runtime.rs:4473`, and upload errors return without rollback at `crates/pcloud-daemon/src/runtime.rs:4499`.

**Impact:** A network or server failure after local rekey leaves daemon memory diverged from server-side crypto state. Users can receive a failure while the process has already adopted the new key material.

**Remediation:** Stage rekey state transactionally, upload first, and commit local state only after server success. On failure, restore the prior crypto shell and KMS cache state. Add a mocked failure test proving the old password and state remain active.

## Medium Findings

### M-01 Signed download tokens are debuggable and plaintext download remains a public knob

**Severity:** Medium

**Evidence:** `SignedDownload` derives `Debug` and contains signed path and `dwltag` at `crates/pcloud-proto/src/http_download.rs:51`. The same file says signed URL tokens must not be logged at `crates/pcloud-proto/src/http_download.rs:17`. `HttpDownloadConfig.use_tls` is public at `crates/pcloud-proto/src/http_download.rs:70`, and false selects plaintext port 80 at `crates/pcloud-proto/src/http_download.rs:291`.

**Impact:** Signed download bearer material can leak through Debug output, and library callers can accidentally force plaintext downloads.

**Remediation:** Implement redacted `Debug` for signed downloads and hide or rename plaintext mode behind an explicit unsafe/test-only constructor. Enforce production TLS at the API boundary.

### M-02 Vault KMS client has no request timeout

**Severity:** Medium

**Evidence:** The blocking reqwest client is built without timeout at `crates/pcloud-kms/src/lib.rs:576`. KMS requests then call blocking `.send()` at `crates/pcloud-kms/src/lib.rs:599`.

**Impact:** A stalled Vault endpoint can indefinitely block crypto unlock or sector encryption/decryption paths, creating a denial-of-service condition.

**Remediation:** Add configurable connect and total request timeouts, plus bounded retries and circuit-breaker behavior. Convert timeout failures into a clear `KmsError::Unreachable`.

### M-03 KMS plaintext DEK cache is global and unbounded

**Severity:** Medium

**Evidence:** The plaintext DEK cache is a global `OnceLock<Mutex<HashMap<...>>>` at `crates/pcloud-kms/src/lib.rs:239`. Expiry cleanup is opportunistic per lookup at `crates/pcloud-kms/src/lib.rs:244`, and insert has no capacity bound at `crates/pcloud-kms/src/lib.rs:256`.

**Impact:** Many distinct wrapped DEKs or contexts can grow memory without bound and keep decrypted key material resident longer than necessary.

**Remediation:** Add a size-bounded LRU with eager expiry, expose explicit cache clearing on lock/logout/shutdown, and zeroize evicted entries.

## Positive Observations

`pcloud-secret` has strong baseline discipline: `SecretString` and `SecretBytes` use zeroizing storage and redacted custom Debug at `crates/pcloud-secret/src/secret_string.rs:35`, `crates/pcloud-secret/src/secret_string.rs:95`, `crates/pcloud-secret/src/secret_bytes.rs:22`, and `crates/pcloud-secret/src/secret_bytes.rs:76`.

TLS transport uses rustls/webpki roots and TLS 1.3-only configuration at `crates/pcloud-proto/src/tls.rs:92`. API server hints are allowlisted before use at `crates/pcloud-proto/src/transport.rs:418`.

Vault store-side writes are stronger than load-side validation: temp file creation, `0600`, fsync, rename, and directory fsync are implemented around `crates/pcloud-daemon/src/vault/file.rs:164`.

## Commands Run

`sed -n '1,240p' pcloud_rev.md`

`find crates -path '*/target' -prune -o -path '*/vendor' -prune -o -type f \( -path '*/src/*' -o -path '*/tests/*' \) | sort | rg 'pcloud-(secret|crypto|kms|config|proto|daemon|backends|backend|sdk|cli)'`

`rg` searches for secret, auth, TLS, KMS, vault, Debug, logging, plaintext, and crypto-share patterns across scoped crates.

`nl -ba` reads of scoped files in `pcloud-secret`, `pcloud-proto`, `pcloud-config`, `pcloud-daemon`, `pcloud-kms`, `pcloud-crypto`, `pcloud-ipc`, and `pcloud-backends`.

## Limitations

This was a static, read-only audit. I did not run builds, tests, fuzzers, packet captures, or live pCloud/KMS integrations. I excluded `target/`, `vendor/`, `.beads/`, `GPTREV/`, `CLAUDEREV/`, and generated tracker output as requested.

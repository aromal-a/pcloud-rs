# Turn 5 Security / Crypto / Transport Review

Read-only review of the current dirty tree after Turn 4 fixes. No files were edited by the review agent.

## Findings

### 1. HIGH - IPC secret frames still retain plaintext, and some one-time secrets are not redacted

Severity: High

Evidence: `Request` still derives `Debug` at `crates/pcloud-ipc/src/methods.rs:260`. `TwoFactorCodeSubmission.value` is a plain `String` at `crates/pcloud-ipc/src/methods.rs:288` and `crates/pcloud-ipc/src/methods.rs:290`. Crypto password-change confirmation codes are plain `String` at `crates/pcloud-ipc/src/methods.rs:344` and `crates/pcloud-ipc/src/methods.rs:358`. `VerifyEmailRestricted.verify_token` is a plain `String` at `crates/pcloud-ipc/src/methods.rs:1129`.

Evidence: even fields already wrapped in `RedactedString` serialize as plaintext because `RedactedString` is `serde(transparent)` at `crates/pcloud-ipc/src/redacted.rs:38` and `crates/pcloud-ipc/src/redacted.rs:39`. IPC encoding uses ordinary `Vec<u8>` at `crates/pcloud-ipc/src/protocol.rs:202`, `:203`, `:222`, and `:227`. IPC read paths also allocate ordinary `Vec<u8>` buffers at `crates/pcloud-ipc/src/transport.rs:906`, `:918`, `:920`, `:926`, `:951`, and `:953`.

Impact: passwords, auth tokens, TFA/recovery codes, verification tokens, and crypto confirmation codes can remain in heap buffers after IPC encode/decode. Some values can also appear in `Debug` output because the field itself is not redacted.

Remediation: convert all secret-like IPC fields, including TFA/recovery values, verification tokens, and confirmation codes, to `RedactedString` or a stronger secret wrapper. Carry serialized request/response payloads in `Zeroizing<Vec<u8>>` or explicitly zeroize after decode/write. Add regression tests that `format!("{:?}", Request::TwoFactorCodeSubmission { ... })` and crypto confirmation requests do not contain supplied secrets.

### 2. HIGH - Binary API `EncodedRequest` still retains plaintext credentials in `params`

Severity: High

Evidence: binary string parameters are owned `String` values at `crates/pcloud-proto/src/binary_api.rs:116` and `:124`. `EncodedRequest.params` is retained "verbatim" at `crates/pcloud-proto/src/binary_api.rs:226`, `:227`, and `:237`. Only `EncodedRequest.bytes` is zeroizing at `crates/pcloud-proto/src/binary_api.rs:245` and `:247`. The encoder copies parameters into the request at `crates/pcloud-proto/src/binary_api.rs:466` and `:471`.

Evidence: credential examples reach this path directly: auth token, old password, and new password are copied into `BinaryParamValue::String` at `crates/pcloud-proto/src/methods/account.rs:278`, `:279`, `:282`, `:283`, `:286`, and `:287`. Folder auth token is copied at `crates/pcloud-proto/src/methods/folder.rs:36`.

Impact: debug output is now redacted, but auth tokens and passwords still survive as ordinary heap strings in the retained decoded parameter view. This violates the master prompt's "no plaintext secret lifetime beyond immediate use" requirement.

Remediation: remove production retention of `params`, or gate it behind `cfg(test)`/dev-only transports. Introduce a secret-aware parameter type that zeroizes string storage. If mock transports need introspection, provide a redacted metadata view instead of storing live plaintext parameters.

### 3. HIGH - TLS revocation modes are configurable but not enforced

Severity: High

Evidence: `TlsRevocationCheck` exposes `StapledPermissive`, `StapledStrict`, and `CrlFile` at `crates/pcloud-config/src/api.rs:50`, `:60`, `:66`, and `:71`. The config field is present at `crates/pcloud-config/src/api.rs:176` and `:188`. `ApiEndpoint::validate` checks TLS mode, host, port, and timeouts at `crates/pcloud-config/src/api.rs:231` through `:278`, but does not enforce or reject revocation settings.

Evidence: the TLS module documents that CRL/OCSP stapling is not performed at `crates/pcloud-proto/src/tls.rs:16`, `:18`, and `:20`. The revocation hook is a no-op at `crates/pcloud-proto/src/tls.rs:86` through `:90`. Runtime TLS clients use `shared_config()` without a revocation policy at `crates/pcloud-proto/src/transport.rs:548` and `crates/pcloud-proto/src/http_download.rs:799`.

Impact: operators can configure strict or CRL-based revocation and still get the same behavior as disabled revocation. This is a false assurance problem for enterprise/FedRAMP deployments.

Remediation: either wire `TlsRevocationCheck` into rustls verification and fail closed for strict/CRL modes, or reject non-disabled revocation modes during validation until implemented. Add a synthetic revoked-certificate test.

### 4. HIGH - Windows WinFSP dynamic load remains search-order hijackable

Severity: High

Evidence: the loader explicitly attempts `LoadLibraryW("winfsp-x64.dll")` at `crates/pcloud-fs/src/platform/winfsp_ffi.rs:622`. The comments acknowledge reliance on Win32 loader search order and PATH/co-location at `crates/pcloud-fs/src/platform/winfsp_ffi.rs:631`, `:632`, and `:633`. The code imports and calls `LoadLibraryW` at `crates/pcloud-fs/src/platform/winfsp_ffi.rs:636`, `:639`, and `:643`.

Impact: if the daemon executable directory, current directory, or PATH segment is attacker-writable, Windows can load a malicious `winfsp-x64.dll`, resulting in code execution as the daemon/service user.

Remediation: use `LoadLibraryExW` with safe search flags, or load from a canonical WinFSP install path under `%ProgramFiles%`. Consider `SetDefaultDllDirectories`, reject relative DLL paths, and add a Windows regression test proving a malicious PATH/co-located DLL is not loaded.

### 5. HIGH - Default crypto build still carries ignored RSA timing-side-channel advisory

Severity: High

Evidence: workspace dependency `rsa = "0.9"` is retained with comments acknowledging active `RUSTSEC-2023-0071` at `Cargo.toml:175`, `:178`, and `:181`. `pcloud-crypto` enables `pclsync-v2` by default at `crates/pcloud-crypto/Cargo.toml:45`, `:47`, and `:52`. The code imports RustCrypto RSA at `crates/pcloud-crypto/src/pclsync_rsa.rs:65` and `:67`, and decrypts with it at `crates/pcloud-crypto/src/pclsync_rsa.rs:289` through `:292`. The advisory is explicitly ignored in `audit.toml:13` through `:20` and `deny.toml:27` through `:40`.

Impact: share unwrap / pclsync compatibility RSA-OAEP decrypt remains exposed to the Marvin timing side-channel class. The exception is documented as risk acceptance, not a fix.

Remediation: replace the decrypt path with a constant-time backend or disable/gate affected share decrypt functionality in enterprise builds until a fixed backend lands. Remove the `cargo audit` and `cargo deny` ignores once remediated.

### 6. MEDIUM - File auth vault parent hardening still fails open on non-owned parent directories

Severity: Medium

Evidence: the vault file itself is checked for regular-file, owner, and mode constraints at `crates/pcloud-daemon/src/vault/file.rs:220`, `:229`, and `:235`. Parent chmod hardening is attempted at `crates/pcloud-daemon/src/vault/file.rs:241` and `:254`. On failure, the code only errors if the parent is owned by the current UID at `crates/pcloud-daemon/src/vault/file.rs:257`, `:262`, and `:267`; otherwise it logs and continues at `crates/pcloud-daemon/src/vault/file.rs:271` through `:275`.

Impact: a vault under a non-owned, weak, or writable parent can violate the owner-only path discipline required by the master prompt. The file mode helps, but the path can still be enumerable or raceable by the parent owner.

Remediation: fail closed unless the parent is owner-matched and `0700`, or allow only explicit safe system-managed parents with strict non-writable checks. Add tests for non-owned `0755` and `0777` parent directories.

### 7. MEDIUM - Web management surface still lacks Host/Origin enforcement

Severity: Medium

Evidence: `serve` only asserts loopback bind at `crates/pcloud-web/src/lib.rs:494` through `:500`. The router has no Host/Origin middleware at `crates/pcloud-web/src/routes.rs:73` through `:89`. Token and CSRF checks exist at `crates/pcloud-web/src/routes.rs:688` through `:714` and `crates/pcloud-web/src/routes.rs:736` through `:757`, but they do not validate Host, Origin, or Referer. Daemon-backed routes call the token gate, for example `crates/pcloud-web/src/routes.rs:135`, `:151`, `:171`, `:261`, `:415`, and `:451`.

Impact: this is no longer the Turn 4 unauthenticated-read issue because token gates are present. The remaining gap is defense-in-depth against DNS rebinding, reverse-proxy mistakes, and unsafe origins if a token is exposed to browser context.

Remediation: add top-level middleware that allowlists `Host` values such as `localhost`, `127.0.0.1`, `[::1]`, and any configured reverse-proxy origin. Reject unsafe `Origin`/`Referer` on mutating routes. Add hostile Host/Origin tests.

## Turn 4 Fixes Verified

- Vault KMS rejects non-HTTPS Vault URLs in config and provider construction.
- Plaintext binary API and signed-download transport reject non-loopback hosts.
- No invalid-certificate override was found.

## Commands / Results

- `cargo test -p pcloud-config crypto_kms --locked`: passed, 9 tests.
- `cargo test -p pcloud-kms --locked`: passed, 9 tests.
- `cargo test -p pcloud-kms --features vault --locked`: passed, 12 tests; 1 live Vault test ignored.
- `cargo test -p pcloud-proto plaintext --locked`: passed, 5 tests.
- `cargo test -p pcloud-proto tls --locked`: passed, 3 tests.
- `cargo test -p pcloud-proto redacts --locked`: passed, 6 tests.
- `cargo test -p pcloud-ipc redacted --locked`: passed, 6 matching tests across unit/security suites.
- `cargo test -p pcloud-ipc --test request_size_cap --locked`: passed, 3 tests.
- `cargo test -p pcloud-web web_token --locked`: passed, 8 tests.
- `cargo test -p pcloud-web csrf --locked`: passed, 4 tests.
- `cargo test -p pcloud-daemon vault --locked`: passed, 28 matching tests.
- `cargo audit --deny warnings --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2025-0134 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141`: passed only with explicit ignores.
- `cargo deny check`: exit 0; advisories/bans/licenses/sources ok, with warnings for duplicate dependencies and unmatched license allowances.

Not run: full workspace test suite, live pCloud tests, live Vault integration, Windows DLL-load exploit test, or network TLS revocation integration.

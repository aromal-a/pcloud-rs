# Turn 4 Security / Crypto / Transport Review

Read-only audit using `pcloud_rev.md` as the master prompt. No files were edited.

## Findings

### C1. HashiCorp Vault KMS can use plaintext HTTP and expose Vault token plus DEK

Severity: Critical

Evidence: `crates/pcloud-config/src/crypto_kms.rs:186` validates Vault config only for non-empty `url`, `transit_key`, and `token_env`; it does not require `https://`. `crates/pcloud-config/src/crypto_kms.rs:285` reads the Vault token from env and `crates/pcloud-config/src/crypto_kms.rs:292` passes the unchecked URL into `HashicorpVault::new`. `crates/pcloud-kms/src/lib.rs:576` builds a reqwest client without `.https_only(true)`, and `crates/pcloud-kms/src/lib.rs:581` stores the URL as-is. `crates/pcloud-kms/src/lib.rs:594` constructs request URLs from that base, `crates/pcloud-kms/src/lib.rs:598` sends `X-Vault-Token`, and `crates/pcloud-kms/src/lib.rs:637` sends the plaintext DEK base64 in the Transit encrypt body.

Impact: a config typo or malicious config can send both the Vault auth token and raw DEK over cleartext HTTP.

Remediation: parse Vault URLs with `url::Url`; reject any non-HTTPS scheme in both `CryptoKmsConfig::validate` and `HashicorpVault::new`; set reqwest `.https_only(true)`; allow plaintext only behind a test-only feature and loopback-only guard; add regression tests for `http://vault`.

### H1. Binary API request objects retain and can debug-print plaintext auth/password material

Severity: High

Evidence: `RedactedProtoString` is redacted and zeroized at `crates/pcloud-proto/src/redacted.rs:41` and `crates/pcloud-proto/src/redacted.rs:88`, but many methods immediately copy secrets into plain `String` parameters. Examples include account auth/passwords at `crates/pcloud-proto/src/methods/account.rs:277`, `crates/pcloud-proto/src/methods/account.rs:283`, `crates/pcloud-proto/src/methods/account.rs:287`, signup password at `crates/pcloud-proto/src/methods/account.rs:348`, and auth tokens via `BinaryParam::string` at `crates/pcloud-proto/src/methods/folder.rs:36`. `BinaryParamValue` derives `Debug`/`Clone` and stores `String` at `crates/pcloud-proto/src/binary_api.rs:113`; `BinaryParam` derives `Debug`/`Clone` at `crates/pcloud-proto/src/binary_api.rs:148`. `EncodedRequest` derives `Debug`/`Clone` and retains both `params` and serialized `bytes` at `crates/pcloud-proto/src/binary_api.rs:205`. Encoding copies secret strings into bytes at `crates/pcloud-proto/src/binary_api.rs:356`, then returns a duplicate `params: params.to_vec()` at `crates/pcloud-proto/src/binary_api.rs:374`.

Impact: accidental debug logs, panic diagnostics, test failures, or core dumps can expose account passwords, auth tokens, or public-link passwords. Heap copies also persist after transport write.

Remediation: remove derived `Debug` from `EncodedRequest`, `BinaryParam`, and `BinaryParamValue` or implement redacted `Debug` keyed by secret parameter names; do not retain plaintext `params` in `EncodedRequest`; store request bytes in `Zeroizing<Vec<u8>>` or a zeroizing newtype; add tests proving `Debug` does not contain auth/password values.

### H2. IPC secret frames use ordinary JSON buffers that are not zeroized

Severity: High

Evidence: `Request` carries secret-bearing variants for IPC at `crates/pcloud-ipc/src/methods.rs:241`, while the redaction wrapper only protects `Debug`/`Drop` at `crates/pcloud-ipc/src/redacted.rs:38` and `crates/pcloud-ipc/src/redacted.rs:136`. Encoding uses `serde_json::to_vec` into a plain `Vec<u8>` at `crates/pcloud-ipc/src/protocol.rs:192`, then copies it into another plain `Vec<u8>` at `crates/pcloud-ipc/src/protocol.rs:217`. Inbound reads allocate `payload` and `bytes` as plain `Vec<u8>` at `crates/pcloud-ipc/src/transport.rs:923` and `crates/pcloud-ipc/src/transport.rs:925`.

Impact: CLI-to-daemon passwords, auth tokens, crypto passwords, and public-link passwords remain in heap allocations after request handling.

Remediation: return and carry `Zeroizing<Vec<u8>>` for encoded frames and inbound request buffers; zeroize temporary JSON payloads immediately after copying/writing; avoid `request.clone()` for secret-bearing requests; add a regression test around secret JSON frame cleanup.

### H3. Web UI exposes sensitive read routes without web-token, Host, or Origin enforcement

Severity: High

Evidence: module docs state the web surface has no auth beyond same-user IPC and relies on loopback plus no CORS at `crates/pcloud-web/src/lib.rs:49`. `WebConfig` documents the token as required for mutating routes only, with read-only routes excluded at `crates/pcloud-web/src/lib.rs:204`. The router exposes `/sync`, `/publinks`, `/activity`, and `/settings` at `crates/pcloud-web/src/routes.rs:79`, `crates/pcloud-web/src/routes.rs:81`, `crates/pcloud-web/src/routes.rs:83`, and `crates/pcloud-web/src/routes.rs:84`. `/api/status` checks `require_web_token` at `crates/pcloud-web/src/routes.rs:146`, but `/sync` calls IPC without token validation at `crates/pcloud-web/src/routes.rs:166`, `/publinks` at `crates/pcloud-web/src/routes.rs:253`, `/activity` returns data without token validation at `crates/pcloud-web/src/routes.rs:404`, and `/settings` exposes the socket path at `crates/pcloud-web/src/routes.rs:437`. The token gate notes read-only routes do not call it at `crates/pcloud-web/src/routes.rs:716`.

Impact: any local process, browser DNS-rebinding origin, or malicious same-user web content path that can reach loopback can enumerate sync roots, public links, activity, and settings.

Remediation: require `X-PCloud-Web-Token` for every route except `/health`, `/livez`, and `/readyz`; validate `Host` against loopback/localhost or configured origin; reject unsafe `Origin`/`Referer` on browser routes; keep CORS disabled but do not rely on CORS as auth.

### H4. Signed downloads and binary transport still expose public TLS-off downgrade knobs

Severity: High

Evidence: `HttpDownloadConfig.use_tls` is public at `crates/pcloud-proto/src/http_download.rs:71`. Download code chooses port `443` or `80` from this flag at `crates/pcloud-proto/src/http_download.rs:291`, then takes a plaintext branch at `crates/pcloud-proto/src/http_download.rs:312`. Resumable range downloads do the same at `crates/pcloud-proto/src/http_download.rs:751` and `crates/pcloud-proto/src/http_download.rs:784`, while adding the signed `dwltag` cookie at `crates/pcloud-proto/src/http_download.rs:762`. Backend construction derives this from `ApiMode` at `crates/pcloud-backends/src/transfer_backend.rs:337`, and FUSE callers can inject arbitrary `HttpDownloadConfig` through `crates/pcloud-fs/src/backend.rs:241`. Binary transport has a public `TransportConfig::with_tls(use_tls: bool, ...)` at `crates/pcloud-proto/src/transport.rs:209`.

Impact: validated production config rejects plaintext defaults, but library/FUSE/SDK callers can still route signed URLs, `dwltag`, and binary API auth over plaintext if they bypass validated profile construction.

Remediation: make TLS-off constructors test/dev-feature gated; make `HttpDownloadConfig.use_tls` private; require a validated transport policy type for SDK/FUSE construction; reject non-TLS signed downloads outside loopback test fixtures.

### H5. Windows WinFSP FFI uses loader search order for `winfsp-x64.dll`

Severity: High on Windows service builds

Evidence: `crates/pcloud-fs/src/platform/winfsp_ffi.rs:627` says loading relies on Win32 loader search order and PATH/co-location. `crates/pcloud-fs/src/platform/winfsp_ffi.rs:635` builds the bare DLL name `winfsp-x64.dll`, and `crates/pcloud-fs/src/platform/winfsp_ffi.rs:639` calls `LoadLibraryW` on that bare name.

Impact: DLL search-order hijacking can load an attacker-controlled `winfsp-x64.dll`, leading to code execution as the daemon user or service account.

Remediation: use `LoadLibraryExW` with restricted search flags; prefer an absolute canonical `%ProgramFiles%\WinFsp\bin\winfsp-x64.dll` path; reject relative paths; optionally verify Authenticode publisher/signature before resolving exports.

### M1. TLS revocation checking is documented but disabled/no-op

Severity: Medium

Evidence: `TlsRevocationCheck::Disabled` is the default and explicitly not FedRAMP-compliant at `crates/pcloud-config/src/api.rs:50` and `crates/pcloud-config/src/api.rs:74`. `pcloud-proto` documents that CRL/OCSP is not performed at `crates/pcloud-proto/src/tls.rs:16`, and the revocation hook is only a placeholder at `crates/pcloud-proto/src/tls.rs:52`. TLS is pinned to TLS 1.3 at `crates/pcloud-proto/src/tls.rs:100`.

Impact: normal WebPKI validation works, but revoked certificates are accepted until roots/intermediates are removed or certs expire.

Remediation: implement `StapledStrict` and `CrlFile` in rustls verifier wiring; make production/FedRAMP profiles fail validation when revocation is disabled; add tests using a synthetic revoked certificate.

### M2. Web-token runtime file write follows/truncates existing paths

Severity: Medium

Evidence: token dir creation and best-effort chmod occur at `crates/pcloud-web/src/lib.rs:316` and `crates/pcloud-web/src/lib.rs:323`; failure to chmod is ignored. The token file is opened with `create(true).truncate(true)` at `crates/pcloud-web/src/lib.rs:330`, then written at `crates/pcloud-web/src/lib.rs:335`. There is no parent owner/mode validation, no symlink check, no `create_new`, and no fsync.

Impact: if `XDG_RUNTIME_DIR` or the token subdirectory is mis-owned or writable, an attacker can race/symlink the token path or cause owner-writable file truncation.

Remediation: validate runtime dir and token dir ownership/mode; fail closed if not owner-only; write to a `create_new` temp file with `0600`, fsync, and rename; use `O_NOFOLLOW`/`symlink_metadata` where available.

### L1. File auth vault soft-fails parent hardening when parent is not owned by the current UID

Severity: Low

Evidence: vault file ownership and mode are enforced at `crates/pcloud-daemon/src/vault/file.rs:221`, `crates/pcloud-daemon/src/vault/file.rs:228`, and `crates/pcloud-daemon/src/vault/file.rs:235`. Parent chmod failure is fatal only when the parent is owned by the current UID at `crates/pcloud-daemon/src/vault/file.rs:253`; if not owned, the code logs a warning and continues at `crates/pcloud-daemon/src/vault/file.rs:271`.

Impact: token file contents remain protected by `0600`, but parent directory listing/traversal and pathname manipulation risk can remain outside the intended owner-only vault discipline.

Remediation: require managed vault parents to be current-user-owned and `0700`; if root/system-owned parents are intentionally supported, require explicit allowlist and strict mode checks.

### L2. Web public-link form derives `Debug` over a cleartext password field

Severity: Low

Evidence: `PublinkCreateForm` derives `Debug` at `crates/pcloud-web/src/routes.rs:268` and contains `password: String` at `crates/pcloud-web/src/routes.rs:274`. It zeroizes on drop at `crates/pcloud-web/src/routes.rs:282`, but derived `Debug` would print the password if the form is logged or included in a panic diagnostic.

Impact: no direct production log site was found for this struct, but the derived formatter violates the repo's secret redaction pattern.

Remediation: remove `Debug` or implement a custom redacted formatter; store the field in a redacted/zeroizing wrapper before handler logic clones it.

## Positive Controls Observed

`SecretString` and `SecretBytes` are redacted, constant-time comparable, and zeroize-on-drop at `crates/pcloud-secret/src/secret_string.rs:35`, `crates/pcloud-secret/src/secret_string.rs:95`, `crates/pcloud-secret/src/secret_string.rs:101`, `crates/pcloud-secret/src/secret_bytes.rs:22`, `crates/pcloud-secret/src/secret_bytes.rs:76`, and `crates/pcloud-secret/src/secret_bytes.rs:82`.

File auth vault writes use `create_new`, `0600`, `sync_all`, rename, and parent fsync at `crates/pcloud-daemon/src/vault/file.rs:170-205`.

Core TLS does not expose an invalid-certificate bypass in reviewed code; search found no `danger_accept_invalid_certs` or `accept_invalid`, and rustls config pins TLS 1.3 at `crates/pcloud-proto/src/tls.rs:100`.

IPC has peer-UID checks and caps: docs at `crates/pcloud-ipc/src/lib.rs:6`, request cap at `crates/pcloud-ipc/src/server.rs:18`, connection caps at `crates/pcloud-ipc/src/transport.rs:71`, and Linux `SO_PEERCRED` at `crates/pcloud-ipc/src/platform/linux.rs:40`.

## Commands And Results

- `cargo test -p pcloud-secret`: passed.
- `cargo test -p pcloud-config crypto_kms`: passed, 6 tests.
- `cargo test -p pcloud-proto tls`: passed, 3 TLS tests.
- `cargo test -p pcloud-proto http_download`: passed, 4 HTTP download tests.
- `cargo test -p pcloud-web web_token`: passed, 4 matching tests across unit/UI tests.
- `cargo test -p pcloud-ipc redacted`: passed, 6 matching tests.
- `cargo test -p pcloud-ipc encode_request`: passed but 0 tests matched.
- `cargo test -p pcloud-daemon vault`: passed, including 18 vault-related unit tests and 10 platform vault tests.
- `cargo test -p pcloud-crypto seal_sector`: passed but 0 tests matched.
- `cargo test -p pcloud-crypto round_trip`: passed, 11 matching tests.
- Full workspace tests, clippy, fmt, dependency advisory scanning, and live KMS/Vault/IdP tests were not run by this review agent.

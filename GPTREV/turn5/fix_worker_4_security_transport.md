# Turn 5 Fix Worker 4: Security And Transport

## Scope

Inputs:

- `GPTREV/turn5/02_security_crypto_transport.md`
- `GPTREV/turn5/04_ipc_daemon_web_config_ops.md`

Ownership honored:

- `crates/pcloud-ipc/src/**`
- `crates/pcloud-proto/src/**`
- `crates/pcloud-config/src/api.rs`
- `crates/pcloud-daemon/src/vault/**`
- `crates/pcloud-fs/src/platform/winfsp_ffi.rs`
- `audit.toml`
- `deny.toml`
- `GPTREV/turn5/fix_worker_4_security_transport.md`

No web-route, sync-runtime, or general documentation files were edited.

## Fixed

- IPC request `Debug` no longer prints request payloads. This redacts TFA,
  recovery, email verification, crypto-confirmation, and any future request
  fields without changing wire formats or out-of-scope call sites.
- IPC client/server transport paths now wrap serialized request/response frame
  buffers in `Zeroizing<Vec<u8>>` where they are held locally.
- Binary API encoded request bytes now zeroize on drop and redact `Debug`.
- `EncodedRequest.params` plaintext retention is limited to debug builds.
  Release builds leave the retained params vector empty and must use
  `EncodedRequest.bytes` for transport execution.
- TLS revocation modes other than `Disabled` now fail config validation until
  CRL/OCSP enforcement is implemented in the transport verifier.
- Unix file vault parent validation now fails closed when the parent is not a
  directory, is not owned by the current user, changes during validation, or
  remains group/other accessible after chmod.
- WinFSP loading no longer relies on `LoadLibraryW("winfsp-x64.dll")` search
  order. It probes canonical Program Files install paths and uses
  `LoadLibraryExW` with restricted dependency lookup flags.

## Changed Paths

- `crates/pcloud-ipc/src/methods.rs`
- `crates/pcloud-ipc/src/protocol.rs`
- `crates/pcloud-ipc/src/client.rs`
- `crates/pcloud-ipc/src/transport.rs`
- `crates/pcloud-proto/src/binary_api.rs`
- `crates/pcloud-config/src/api.rs`
- `crates/pcloud-daemon/src/vault/file.rs`
- `crates/pcloud-fs/src/platform/winfsp_ffi.rs`
- `GPTREV/turn5/fix_worker_4_security_transport.md`

## Verification

Passed:

- `cargo test -p pcloud-ipc request_debug --locked`
- `cargo test -p pcloud-ipc --tests --locked`
- `cargo test -p pcloud-proto plaintext_param_retention_is_not_enabled_for_release_builds --locked`
- `cargo test -p pcloud-proto --lib plaintext_param_retention_is_not_enabled_for_release_builds --release --locked`
- `cargo test -p pcloud-proto --tests --locked`
- `cargo test -p pcloud-config tls_revocation --locked`
- `cargo test -p pcloud-config --tests --locked`
- `cargo check -p pcloud-fs --locked`
- `cargo audit --deny warnings --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2025-0134 --ignore RUSTSEC-2024-0436 --ignore RUSTSEC-2025-0141`
- `cargo deny check`

Blocked:

- `cargo test -p pcloud-daemon vault --locked` did not compile because of an
  out-of-scope error in `crates/pcloud-daemon/src/runtime.rs:3140`, where
  `SecretString::new(result.auth_token)` receives an existing `SecretString`
  instead of a `String`.
- `cargo check -p pcloud-fs --target x86_64-pc-windows-gnu --locked` was
  blocked by the host missing `x86_64-w64-mingw32-gcc`, required by `ring`.

## Unresolved

- RSA Marvin remains advisory-only in this turn. I did not find a safe,
  in-scope constant-time RSA replacement that could be introduced without
  larger protocol/client changes, so the existing audit/deny advisory handling
  remains the practical mitigation record.
- Request field types were not changed to `RedactedString` because that would
  require coordinated updates to out-of-scope CLI/runtime call sites. The
  implemented mitigation is fail-safe request-level `Debug` redaction plus
  local zeroizing of serialized IPC buffers.

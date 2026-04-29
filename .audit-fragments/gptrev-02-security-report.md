# Stream G2 — Security, Secrets & Transport: Fix Report

Audit source: `GPTREV/02_security_secret_transport.md`
Date: 2026-04-26
Agent: Stream G2

## Triage Summary

| Finding | Severity | Decision | Action |
|---------|----------|----------|--------|
| HIGH-01 Runtime auth uses file-vault directly | HIGH | Defer | Large refactor carrying `Box<dyn PlatformVault>` into RuntimeShell; tracked under bd-xplat-windows / vault migration phase. |
| HIGH-02 Wire-capture persists plaintext frames | HIGH | Out of scope | `pcloud-proto/src/transport.rs` not in allowed file scope (G1 domain). Recommend compile-gating behind `#[cfg(feature = "wire-capture")]` in a future pass. |
| HIGH-03 TwoFactorCodeSubmission.value is raw String | HIGH | **Fixed** | Changed `value` from `String` to `RedactedString` in `methods.rs`. Updated all 7 call sites across runtime.rs, lib.rs (×3), commands.rs, main.rs, live_auth.rs, proptest, live-e2e. |
| HIGH-04 PasswordLoginOutcome tokens are Debug-printable Strings | HIGH | Defer | `pcloud-proto/src/auth_api.rs` protocol structs; short-lived, not stored long-term; G1 scope. |
| HIGH-05 Resilient transport not wired | HIGH | Out of scope | Backend refactor; G1/G7 scope. |
| HIGH-06 Windows IPC read/write timeouts are no-ops | HIGH | **Annotated** | Added explicit security-impact comment in `platform/windows.rs` documenting local-DoS risk, overlapped I/O remediation path, and bd-xplat-windows tracking reference. |
| MEDIUM-01 bootstrap read_secret_file extra heap copy | MEDIUM | **Fixed** | Eliminated `buf.clone()` in `bootstrap.rs::read_secret_file`. Now consumes `buf` via `String::from_utf8(buf)`, zeroizes on UTF-8 failure, and trims in-place without a second allocation. The only retained copy is wrapped immediately into `SecretString`. |
| MEDIUM-02 validate_local_sync_path not called in runtime | MEDIUM | Out of scope | Sync domain (G4); touching `runtime.rs::add_sync_root` is forbidden for this stream. |
| MEDIUM-03 Remote path traversal | MEDIUM | Out of scope | `sync_backend.rs` (G4 domain). |
| MEDIUM-04 max_parser_frame_bytes not wired | MEDIUM | Out of scope | `pcloud-proto/src/transport.rs` (G1 domain). |
| MEDIUM-05 File vault parent dir warns instead of failing | MEDIUM | Over-cautious | Current code DOES fail closed when parent is owned by current user and chmod fails (lines 262–270 of vault/file.rs). The warn-only path is explicitly gated to `parent not owned by current uid` — correct and intentional per audit-06 LOW pcloud-rs-ncx.80-b. |
| MEDIUM-06 Bearer codes leak into audit strings | MEDIUM | **Partially fixed** | Upload link full `link` URL removed from `audited_response` rendered string in `runtime.rs` ~5165. The `code` short identifier is retained (it is already user-facing). Public-link `code` at lines 974 and 4835 is accepted risk (it is a short display identifier, not the full bearer URL). |
| MEDIUM-07 TLS revocation advertised but not enforced | MEDIUM | **Annotated** | Added explicit `MEDIUM-07 / pcloud-rs-t9o: not yet implemented` inline comments to all three `TlsRevocationCheck` variants (`StapledPermissive`, `StapledStrict`, `CrlFile`) in `api.rs`. Module already had thorough prose; per-variant markers prevent operator misreading. |

## Files Modified

- `crates/pcloud-ipc/src/methods.rs` — `TwoFactorCodeSubmission.value`: `String` → `RedactedString`
- `crates/pcloud-daemon/src/runtime.rs` — two `SecretString::new(value)` → `SecretString::new(value.into_string())`; upload link URL redacted from audit string
- `crates/pcloud-daemon/src/lib.rs` — three test `TwoFactorCodeSubmission` construction sites updated
- `crates/pcloud-cli/src/commands.rs` — `TwoFactorCodeSubmission` value field updated
- `crates/pcloud-cli/src/main.rs` — `TwoFactorCodeSubmission` value field updated
- `crates/pcloud-daemon/tests/live_auth.rs` — `TwoFactorCodeSubmission` value field updated
- `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs` — `TwoFactorCodeSubmission` value field updated
- `crates/pcloud-live-e2e/tests/common/mod.rs` — `TwoFactorCodeSubmission` value field updated
- `crates/pcloud-daemon/src/bootstrap.rs` — `read_secret_file` extra heap copy eliminated
- `crates/pcloud-ipc/src/platform/windows.rs` — HIGH-06 security note added to `set_read_timeout` / `set_write_timeout`
- `crates/pcloud-config/src/api.rs` — MEDIUM-07 not-implemented markers added to `TlsRevocationCheck` variants

## Verification

```
cargo check -p pcloud-secret -p pcloud-ipc -p pcloud-config -p pcloud-auth   → Finished, 0 errors
cargo check -p pcloud-daemon                                                    → Finished, 0 errors
cargo test  -p pcloud-secret -p pcloud-ipc -p pcloud-config --lib             → 19 passed, 0 failed
```

`pcloud-cache` has a pre-existing `E0063` error (missing fields in `StagingCache`)
unrelated to this stream; it is a parallel-agent concern (G4/G5).

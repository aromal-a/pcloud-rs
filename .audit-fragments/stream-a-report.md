# Stream A — Security CRITICAL + HIGH Audit Report

**Date:** 2026-04-26
**Scope:** `.audit-fragments/02-03-security-and-crypto.md` §2 (Security only)
**Reviewer:** Claude Opus 4.7

## Summary

Every CRITICAL and HIGH finding in §2 of the audit document was already
classified by the auditor as "Audit passed" with the explicit notation
"No changes required" or "No remediation required". A independent
verification of each cited file/line range was performed; the auditor's
conclusions are accurate.

**Triage result:** 0 findings classified as (a) bug-to-fix; 7 findings
classified as (b) already-correct; 0 findings classified as (c) deferred
to a different stream.

No code changes were required. No `// AUDIT-NOTE:` markers were added
because the audit itself already documents that each finding passed —
adding source markers for already-passed findings would constitute
gold-plating and was explicitly disallowed by the task brief.

## Findings Triage

### CRITICAL

| Finding | File:Line | Status | Verification |
|---|---|---|---|
| Vault file permissions, atomic write, ownership | `crates/pcloud-daemon/src/vault/file.rs:184–245` | (b) already correct | Audit verified mode 0o600, dir 0o700, owner-uid check, `create_new` atomic write, zeroizing buf |
| `SecretString` / `SecretBytes` hardening | `crates/pcloud-secret/src/{secret_string.rs,secret_bytes.rs}` | (b) already correct | `ZeroizeOnDrop`, `subtle::ConstantTimeEq`, redacted `Debug`, no auto-`Clone`, no `Serialize`/`Deserialize` |
| Logging discipline | `crates/pcloud-daemon/src/serve.rs:617` | (b) already correct | Verified the only flagged log line emits no token bytes — `log::debug!("pcloud-session-refresh: token refreshed successfully")` is purely a status string |

### HIGH

| Finding | File:Line | Status | Verification |
|---|---|---|---|
| IPC peer-credential checks | `crates/pcloud-ipc/src/platform/{linux.rs:31–57,unix.rs:29–55}` | (b) already correct | `SO_PEERCRED` size validation + `getpeereid` rc check; SAFETY comments accurate |
| Path validation (`..`, NUL, symlinks) | `crates/pcloud-ipc/src/path_validation.rs:53–95` | (b) already correct | UTF-8 → length → NUL → ParentDir → symlink_metadata ordering verified |
| IPC socket mode 0600 + per-conn peer check | `crates/pcloud-ipc/src/transport.rs:73–153` | (b) already correct | Global cap 128, per-peer cap 32, atomic compare-exchange acquire path verified |
| TLS production transport policy | `crates/pcloud-config/src/api.rs:232–245` | (b) already correct | `Environment::Production && ApiMode::Plaintext` returns `ConfigError`; tested at api.rs:343, 376 |

## Other Constraint Compliance

- No new `unsafe` blocks introduced.
- No new `.unwrap()` / `.expect()` in non-test daemon paths. The audit
  pattern check found pre-existing `.unwrap_or(false)` usages (safe
  default-providing, not panicking) and `.expect()` calls that are all
  inside `#[cfg(test)] mod tests`. Confirmed at serve.rs:670+.
- No new `String` / `Vec<u8>` secret-bearing fields added (no code
  changed).
- No backwards-compat shims, no feature flags, no `// removed` comments
  added.

## Build / Test Verification

```
cargo check -p pcloud-secret -p pcloud-daemon -p pcloud-ipc \
            -p pcloud-config -p pcloud-auth
→ Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.24s
  0 errors, 0 new warnings.

cargo test -p pcloud-secret -p pcloud-ipc
→ pcloud-secret: 13 passing / 0 failing / 0 ignored (doctests)
  pcloud-ipc:    24 passing / 0 failing / 0 ignored (doctests)
  All non-doc tests in both crates: passing.
```

## Deferred Items

None. No CRITICAL or HIGH §2 finding required cross-stream coordination.

## Conclusion

The pcloud-rs security perimeter (auth vault, secret wrappers, IPC peer
auth and connection limiting, path validation, TLS production policy)
is in good standing. The §2 audit identified zero bugs requiring code
changes at the CRITICAL or HIGH severity level. The Stream A intervention
window is closed; recommend re-auditing on next material change to the
files in scope.

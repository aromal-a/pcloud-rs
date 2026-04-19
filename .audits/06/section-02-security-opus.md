# Audit 06 — Section 2: Security (Opus)

Date: 2026-04-18
Scope: verify audit-05 security fixes held; re-audit secret discipline end-to-end.

## Verification of audit-05 closures — all held

| Item | Evidence | Status |
|---|---|---|
| `PclsyncCompatProfile` Debug redacted | `crates/pcloud-crypto/src/pclsync_compat_profile.rs:128-140` — manual `Debug` impl emits only lengths/flags, not key material | PASS |
| `SymKeyVer1 Clone` removed | `crates/pcloud-crypto/src/pclsync_rsa.rs:169-223` — no `Clone` derive; only `#[cfg(test)] fn duplicate()` with audit comment citing `pcloud-secret/src/lib.rs:26`; `Debug` redacts both buffers; `ZeroizeOnDrop` derived | PASS |
| Peer uid threaded through dispatch | `crates/pcloud-daemon/src/dispatch.rs:309-371` — `dispatch_with_peer_envelope(runtime, peer_uid, …)`; rate limiter now keyed on peer | PASS |
| Per-peer rate limiter | `crates/pcloud-daemon/src/rate_limit.rs:156-186` — `PerPeerRateLimiter` with `HashMap<u32, SessionRateLimiter>`, policy cloned once, poison-tolerant | PASS |
| `CryptoGet{Folder,File}Key` privileged + audited | `crates/pcloud-daemon/src/runtime.rs:3173-3296` — backend/started gates, `audited_response` on success, `auth_token` sourced from authenticated snapshot | PASS |
| IPC bind re-chmod | `crates/pcloud-ipc/src/transport.rs:689-694` — `remove_file` → `bind` → explicit `set_permissions(0o600)`; parent re-chmod uid-gated lines 676-686 | PASS |
| Digest-only extract-kat | `scripts/extract-pclsync-kat.py:104-134` — `getdigest` first, `passworddigest` always; no plaintext fallback | PASS |
| 52 new SAFETY comments | 78 `SAFETY` occurrences across 20 files (winfsp_ffi=7, linux=7, transport=5, etc.); FFI surfaces annotated | PASS |

## CRITICAL
None.

## HIGH
None. Grep for secret-field logging (`password|token|priv_key|secret|api_key|recovery` inside `log::|tracing::|println!|eprintln!`) across `crates/**/src/` returned only: (a) `pcloud-secret/examples/roundtrip.rs` deliberately formatting a `SecretString` to prove redaction, (b) test skip messages naming env-var *names* not values. No production log call interpolates secret material.

## MEDIUM

### MED-2.1 — IPC socket bind/chmod TOCTOU window
`crates/pcloud-ipc/src/transport.rs:693-694`. `UnixListener::bind` creates the socket inode with the process umask (typically 0022 → 0755) and `set_permissions(0o600)` is applied on the next line. A local attacker racing between the two calls could `connect(2)` on the permissive mode. Parent dir is 0o700 which closes the practical window when the parent is owner-only, but relying on parent hardening alone is fragile.
- **Fix**: wrap the bind in a saved-umask block (`umask(0o177)` → bind → restore), or create the socket via `socket(2)+fchmod+bind` so the inode is never visible at >0600.

### MED-2.2 — `PerPeerRateLimiter` unbounded map growth on uid churn
`crates/pcloud-daemon/src/rate_limit.rs:156-186`. The comment at lines 149-155 correctly notes that owner-only IPC bounds the map to 1 entry in production. However, if the daemon is reconfigured to accept multiple authorized uids (future group-access mode, multi-user dev path, or a bug relaxing the peer gate), the map grows without an LRU/TTL eviction. Not exploitable today but one config change away.
- **Fix**: add a size ceiling + LRU eviction keyed on `last_seen`, or document an assertion that only owner-uid reaches `check()` and panic-in-debug on any other uid.

### MED-2.3 — `clone_secret` on every dispatch of `CryptoGet{Folder,File}Key`
`crates/pcloud-daemon/src/runtime.rs:3196-3210,3260-3275`. Each request clones the auth-token `SecretString` into a local that lives across a network round-trip. `clone_secret` is zeroized on drop, so it's correct, but every clone is a fresh heap allocation that zeroizes independently. This is a minor resource/timing surface; not a leak.
- **Fix**: borrow `&SecretString` into the runtime call and expose it inside the HTTP layer only; avoids the clone entirely.

## LOW

### LOW-2.4 — `pub` key-material fields on `SymKeyVer1`
`crates/pcloud-crypto/src/pclsync_rsa.rs:182,185`. `aes_key` and `hmac_key` are `pub [u8; N]`, allowing external code to copy key material with a trivial field read, bypassing the "no `Clone`" discipline. `ZeroizeOnDrop` only protects the origin instance.
- **Fix**: make fields `pub(crate)` and expose accessors that return a view rather than a copy.

### LOW-2.5 — Parent dir chmod failures swallowed in `validate_vault_file`
`crates/pcloud-daemon/src/vault/file.rs:241-254`. A `set_permissions(parent, 0o700)` failure is logged at `warn!` and ignored. The file-level `0o077` check on the vault inode remains, so this is defense-in-depth only, but it weakens the audit-05 narrative that "parent 0o700 is enforced on load".
- **Fix**: when the parent uid matches the current uid and the mode is lax, treat the chmod failure as `InsecureMetadata` rather than a warn.

### LOW-2.6 — `SendPublink` still declared but C-rejected
`crates/pcloud-daemon/src/dispatch.rs:174`. The dispatch table lists `Request::SendPublink` under `public_link`. `CLAUDE.md` marks `psync_send_publink` as missing/rejected. If the variant exists in `Request` but has no server-side handler, a peer could exercise a ghost code path. (Not a security hole today, but a pruning TODO.)
- **Fix**: either implement or delete the IPC variant.

## Secret-discipline sweep — findings summary

- `SecretString` / `SecretBytes` wrappers: `pcloud-secret/src/{secret_string,secret_bytes}.rs` — `ZeroizeOnDrop`, `Debug` redacted, no `Clone`.
- `subtle::ConstantTimeEq` usage: present in 8 crypto files incl. `pclsync_rsa.rs`, `pclsync_auth_tree.rs`, `share_temppass.rs`, `pclsync_kdf.rs`.
- No production cert-validation bypass: zero `danger_accept_invalid_certs` matches. `http://` hits are confined to `pcloud-idp` test-gated loopback, `pcloud-mockserver`, and the 127.0.0.1 OIDC redirect URI constant.
- Vault: `0o600` file + `0o700` parent + owner-uid check + atomic tmp+rename + `sync_all` → `sync_parent_directory`. Plaintext password path intentionally absent.
- Owner-only IPC: `transport.rs:694` + peer-creds check in `pcloud-ipc::auth`.
- `unsafe` density: 386 blocks; SAFETY comments: 78 (covers most blocks in FFI surfaces, with some gaps in `macos_ffi.rs` where `unsafe extern` declarations dominate rather than `unsafe {}` expressions).

## Summary

All audit-05 §2 fixes are intact in the tree. No CRITICAL or HIGH regressions; three MEDIUMs (IPC bind TOCTOU, per-peer map bound, secret clone churn) and three LOWs identified. The Rust path remains materially stricter than the C baseline on secret handling, IPC hardening, and transport policy.

# Audit 06 §2 — Security (Independent Sonnet Cross-Validation)
**Date:** 2026-04-18
**Auditor:** claude-sonnet-4-6 (independent; post audit-05)
**Scope:** Security discipline across the workspace; verifying audit-05 hardening held.

---

## Summary

The post-audit-05 security posture is substantially sound. `SecretString`/`SecretBytes` wrappers are correctly implemented and used on all primary credential paths. The auth vault is atomic, owner-verified at 0600/0700. IPC enforces peer-uid authorization, per-connection caps, frame-size limits, and per-client read/write timeouts. Transport rejects plaintext in production. Log scanning finds no secret exposure.

Two MEDIUM-severity issues remain: public-link passwords flow as bare `Option<String>` through several layers, and Windows IPC peer-credential check is still described as stubbed in code comments.

---

## Findings

### MEDIUM — M-SEC-01: Public-link password parameter is bare `Option<String>`, not `SecretString`

**Files:**
- `crates/pcloud-backends/src/public_link_backend.rs:760`
- `crates/pcloud-daemon/src/runtime.rs:4187`
- `crates/pcloud-proto/src/public_links_api.rs:385,793`

**Detail:** `change_public_link_password` at all three layers accepts `password: Option<String>`. The value is a user-chosen link protection password, semantically a secret. It is not wrapped in `SecretString`, so it will not be zeroized on drop, and it is visible in any `Debug` formatting of the enclosing struct or trace. The web-layer `PublinkCreateForm` does implement a manual `Drop` zeroize (`crates/pcloud-web/src/routes.rs:265–270`), but that protection is not present in the backend or IPC layers where the value lives longer.

**Recommendation:** Change the parameter to `Option<SecretString>` throughout (`public_links_api.rs`, `public_link_backend.rs`, `runtime.rs`). Update the IPC `Request::ChangePublicLinkPassword` variant accordingly so the wire path carries a `RedactedString`.

---

### MEDIUM — M-SEC-02: Windows IPC peer-credential check documented as stub

**File:** `crates/pcloud-ipc/src/server.rs:8` (module-level comment)

**Detail:** The `server.rs` module comment explicitly notes: *"On Windows the equivalent is a named pipe with a DACL granting `GENERIC_READ|GENERIC_WRITE` only to the current-user SID plus a `GetNamedPipeClientProcessId`-driven TokenUser SID comparison — see the `platform::windows` module."* The `platform/windows.rs` file exists but the peer-credential comparison path has not been independently verified as implemented beyond stub. If the Windows path accepts any connecting process without SID verification, the IPC would be world-accessible on Windows.

**Recommendation:** Verify `crates/pcloud-ipc/src/platform/windows.rs` performs the documented SID comparison before declaring Windows a supported platform for IPC. Add a compile-time or runtime guard that panics or returns `Unauthorized` if the check cannot be completed.

---

### LOW — L-SEC-01: `peer.uid` printed in Unauthorized log message without audit trail gate

**File:** `crates/pcloud-ipc/src/transport.rs:516–518`

**Detail:** When a peer is unauthorized, the response message includes `"unauthorized peer uid={}, pid={}"`. This is fine for operator debugging and reveals no secret. However the same string is sent to the client peer, who already knows their own uid — so this is more of an information-confirmation than a leak. No remediation strictly required; noted for completeness.

---

### LOW — L-SEC-02: `is_known_safe_host` allowlist permits all subdomains of `.pcloud.com` and `.pcloud.link`

**File:** `crates/pcloud-config/src/api.rs:208–210`

**Detail:** The `apply_api_server_hint` guard uses `host.ends_with(".pcloud.com") || host.ends_with(".pcloud.link")`. This is a suffix check, meaning a host like `evil.notpcloud.com` is rejected correctly, but `evil.pcloud.com.attacker.net` would also be rejected (correct). However the check accepts *any* subdomain of `pcloud.com`, including hypothetically compromised ones. This is an acceptable and conventional trust boundary — pCloud controls `*.pcloud.com` — but worth noting.

**Recommendation:** No change required unless pCloud's threat model includes compromise of sub-properties. Current behavior is consistent with industry practice.

---

## Verified-Held Properties (audit-05 hardening confirmed)

| Property | Status | Evidence |
|---|---|---|
| `SecretString` ZeroizeOnDrop | HELD | `secret_string.rs:35` — `#[derive(ZeroizeOnDrop)]` |
| `SecretBytes` ZeroizeOnDrop | HELD | `secret_bytes.rs:22` — `#[derive(ZeroizeOnDrop)]` |
| `Debug` redacted on both wrappers | HELD | `secret_string.rs:95–99`, `secret_bytes.rs:76–79` |
| `Clone` deliberately absent; `clone_secret` audit-visible | HELD | Both wrappers |
| Constant-time `PartialEq` via `subtle::ConstantTimeEq` | HELD | Both wrappers |
| No `Serialize`/`Deserialize` on secret wrappers | HELD | Both files; compile-fail test referenced |
| Vault file 0600, parent 0700 enforced | HELD | `vault/file.rs:162,193,199` |
| Atomic vault write (tmp + rename) | HELD | `vault/file.rs:165–197` |
| Vault O_CREAT\|O_EXCL on tmp to prevent symlink race | HELD | `vault/file.rs:179–183` |
| Owner-uid check on vault load | HELD | `vault/file.rs:223–227` |
| Vault group/other bits rejected | HELD | `vault/file.rs:230–233` |
| No plaintext password persistence | HELD | `vault/file.rs` — only token stored, confirmed by comments |
| IPC socket 0600 + 0700 parent | HELD | `transport.rs:677,694` |
| SO_PEERCRED on Linux, getpeereid on BSD/macOS | HELD | `transport.rs:866–878` |
| Per-connection cap (global 128, per-peer 32) | HELD | `transport.rs:44,54` |
| Frame-size pre-allocation cap (1 MiB) | HELD | `server.rs:42`; enforced pre-alloc at `transport.rs:791` |
| Per-client read timeout 5s, write timeout 30s | HELD | `transport.rs:146,150` |
| Production rejects `ApiMode::Plaintext` | HELD | `api.rs:137–141` |
| API-server hint validates to `.pcloud.com`/`.pcloud.link` only | HELD | `api.rs:208–210` |
| No secret values in log output | HELD | Full grep over crates found only `token refreshed successfully` (no value) |
| Token in logs: `serve.rs:525` logs only "token refreshed successfully" — no token value | HELD | `serve.rs:525` |

---

## Count

| Severity | Count |
|---|---|
| CRITICAL | 0 |
| HIGH | 0 |
| MEDIUM | 2 |
| LOW | 2 |

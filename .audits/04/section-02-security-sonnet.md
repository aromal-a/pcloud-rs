# Section 2: Security Audit — pcloud-rs
**Date:** 2026-04-18  
**Auditor:** Claude Sonnet 4.6 (parallel with Opus)  
**Scope:** Secret discipline, auth vault, IPC credential checks, TLS enforcement, sensitive-data exposure.

---

## Findings by Severity

### MEDIUM [3]
### LOW [2]

---

## Detailed Findings

### SEC-M1 — MEDIUM: `setup_crypto` accepts `String` password parameter across internal call boundary

**File:** `crates/pcloud-daemon/src/runtime.rs:2774`

```rust
fn setup_crypto(&mut self, password: String, hint: Option<String>) -> Response {
    // ...
    let secret = SecretString::new(password);
```

The IPC dispatch at line 572–575 correctly extracts from `RedactedString` into a `Zeroizing<String>` then calls `.clone()` before passing to `setup_crypto`. This means the password lives in an unprotected `String` on the stack for the duration of the call:

```rust
let password = Zeroizing::new(password.into_string());
self.setup_crypto((*password).clone(), hint)  // clone creates a non-Zeroizing String
```

The `.clone()` at line 574 allocates a bare `String` that is passed to `setup_crypto` without zeroize-on-drop. The `Zeroizing` wrapper on the outer binding does not protect the clone. `setup_crypto` then wraps it in `SecretString::new(password)`, but only after the bare `String` already exists. The same pattern likely applies to `CryptoUnlock` and `CryptoChangePassword`.

**Remediation:** Change `setup_crypto` signature to accept `SecretString` directly; eliminate the intermediate bare `String` clone. Use `SecretString::new(password.into_string())` at the dispatch site without `.clone()`.

---

### SEC-M2 — MEDIUM: `web_token` stored and passed as bare `String` (not `SecretString`)

**Files:**  
- `crates/pcloud-web/src/lib.rs:175,245`  
- `crates/pcloud-web/src/routes.rs:258`

The web management session token is generated with 32 bytes of OS CSPRNG entropy and written to a 0600 file — good. However, it is stored as `pub web_token: String` and `pub web_token: Arc<String>` in `WebConfig`/`AppState`. It is compared in route handlers without constant-time equality. A timing oracle on the HTTP `X-PCloud-Web-Token` comparison could leak prefix bytes of the token.

Additionally, the `PublinkCreateForm` struct at routes.rs:258 deserializes public-link passwords as `password: String` without a secret wrapper. This is an HTTP form field and persists in the Axum form extractor until the function returns; a heap snapshot during form processing could capture the public-link password in plain.

**Remediation:** Store `web_token` as `SecretString` (or at minimum compare with `subtle::ConstantTimeEq`). Wrap `PublinkCreateForm::password` in a redacted newtype; zeroize on drop.

---

### SEC-M3 — MEDIUM: `public_link_backend.rs` `password` parameter uses `Option<String>`, not `SecretString`

**Files:**  
- `crates/pcloud-backends/src/public_link_backend.rs:764,1003`  
- `crates/pcloud-proto/src/public_links_api.rs:385,793`

Public-link passwords are passed as `Option<String>` from the IPC dispatch layer to the backend and into the proto encoder. These are user-chosen secrets protecting shared links. They are not wrapped in a secret type and may persist in heap memory until GC at arbitrary future times. Debug output of the backend struct could leak them.

**Remediation:** Introduce `Option<SecretString>` for public-link passwords throughout the stack; use a serde-skip or redacted-serialize wrapper at the proto boundary where wire encoding requires a plain `&str`.

---

### SEC-L1 — LOW: `IpcServer::bind` does not verify pre-existing parent directory ownership

**File:** `crates/pcloud-ipc/src/transport.rs:395–409`

When the runtime directory (`socket_path.parent()`) already exists, the code skips the `set_permissions(0o700)` call:

```rust
let parent_missing = !parent.exists();
fs::create_dir_all(parent)?;
if parent_missing {
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
}
```

If a prior run left the directory with relaxed permissions (or if another process created it first), the socket will be placed inside a world-readable parent, undermining the owner-only socket model. The socket file itself is chmoded 0600 afterward, but a world-readable parent directory allows non-owner users to `stat` or `connect` the socket path (connect will be rejected by peer-uid check, but the path is enumerable).

**Remediation:** Always call `set_permissions(parent, 0o700)` regardless of whether the directory was just created. Alternatively, verify the existing directory's mode and ownership before proceeding, and fail if they do not meet the 0700/owned-by-self requirements.

---

### SEC-L2 — LOW: `insecure-plaintext-exchange` feature flag not documented as forbidden in production

**File:** `crates/pcloud-idp/Cargo.toml:19`

The feature `insecure-plaintext-exchange = []` exists in the IdP crate. The module doc (`exchange.rs:29`) states this flag is required to enable plaintext HTTP in the OIDC exchange path and that it is "gated `#[cfg(any(test, feature = "insecure-plaintext-exchange"))]` so production builds cannot disable TLS." This is correct for code compiled with default features. However, there is no CI gate or workspace-level deny to prevent a packaging maintainer from accidentally enabling this feature in a release build.

**Remediation:** Add a `[features]` deny rule in `deny.toml` or a CI step that verifies `insecure-plaintext-exchange` is not in the feature set of any release artifact. Document in the feature's `Cargo.toml` comment that it must never appear in release profiles.

---

## Strengths Noted

The following areas are well-implemented and should be preserved:

- **`SecretString` / `SecretBytes`:** `#[derive(ZeroizeOnDrop)]`, no `Serialize`/`Deserialize`, constant-time `PartialEq` via `subtle::ConstantTimeEq`, and redacted `Debug`. Compile-fail test enforces the no-serialize invariant.
- **Auth vault (`vault/file.rs`):** Atomic write (tmp+rename), O_CREAT|O_EXCL on tmp file, 0600 file / 0700 parent, `symlink_metadata` to block symlink attacks, ownership and mode validation before load, manual zeroize of intermediate buffer before `SecretString` wrap.
- **IPC peer credentials:** `SO_PEERCRED` (Linux) and `getpeereid` (BSD/macOS) enforced on every accepted connection; uid mismatch returns `Unauthorized` before any request bytes are read; no per-request capability escalation path exists.
- **IPC DoS mitigations:** 1 MiB payload cap pre-allocation, 5s read timeout, 30s write timeout, 128-connection cap with RAII guard, oversized-frame connections closed without response.
- **TLS enforcement:** `ApiEndpoint::validate` hard-rejects `ApiMode::Plaintext` in `Environment::Production`; `apply_api_server_hint` allowlists only `.pcloud.com` / `.pcloud.link`; `danger_accept_invalid_certs` not found in any production path.
- **No secret logging:** Grep across all log call sites found no password/token values emitted. `Request::PasswordSubmission` and crypto variants use `RedactedString` which redacts `Debug`.
- **Web token:** Generated from OS CSPRNG (32 bytes → 64 hex chars), written to `XDG_RUNTIME_DIR/pcloud-daemon/web-token` at 0600. Token value is explicitly not logged to stderr.
- **IPC `Request` transit rationale:** The conscious decision to use `String` for IPC-wire secret fields (with `RedactedString` Debug wrapper) is documented in-source at `methods.rs:243–259` with the short-lifetime justification and a regression-watch comment.

---

## Remediation Priority

| ID | Severity | Action |
|----|----------|--------|
| SEC-M1 | MEDIUM | Eliminate bare `String` clone of crypto password in runtime dispatch |
| SEC-M2 | MEDIUM | Use constant-time comparison for `web_token`; wrap publink form password |
| SEC-M3 | MEDIUM | Propagate `Option<SecretString>` for public-link passwords |
| SEC-L1 | LOW | Always enforce 0700 on IPC parent directory, even when pre-existing |
| SEC-L2 | LOW | CI/deny gate to prevent `insecure-plaintext-exchange` in release builds |

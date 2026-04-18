# Section 2: Security Audit
## Date: 2026-04-17
## Scope

Security audit of the pcloud-rs Rust rewrite focused on: secret hygiene, auth
vault, local IPC surface, outbound transport policy, crypto, memory safety
(`unsafe`), input validation, logging, and DoS mitigations.

Files reviewed:
- `crates/pcloud-secret/src/{secret_string,secret_bytes}.rs`
- `crates/pcloud-daemon/src/auth_vault.rs`
- `crates/pcloud-daemon/src/vault/{mod,file,dpapi,keychain,secret_service}.rs`
- `crates/pcloud-ipc/src/{lib,server,transport,client,protocol,methods,redacted,auth,path_validation,platform/{mod,linux,unix,windows}}.rs`
- `crates/pcloud-daemon/src/{runtime,dispatch,serve,signals,rate_limit,transport_factory,metrics_server}.rs`
- `crates/pcloud-proto/src/transport.rs`
- `crates/pcloud-config/src/api.rs`
- `crates/pcloud-crypto/src/{lib,content,keys}.rs`

## Findings summary

### CRITICAL [0]
_(none)_

### HIGH [4]
- H1. `pcloud-proto` method structs carry auth tokens / passwords as plain `String` (fan-out across ~30 files).
- H2. Outbound transport: `total_request_timeout` is declared but NOT enforced by the production deadline loop; each stage uses `read_timeout` only.
- H3. `ChangePublicLinkPassword.password` is `Option<RedactedString>` on the wire but delivered to the backend as a `String` and there is no `SecretString` coercion for the public-link password.
- H4. No per-peer rate-limit category for password/crypto-unlock attempts — wrong-password attempts fall into the "Medium" bucket alongside routine reads; an online brute-force against `UnlockCrypto` / `SubmitPassword` is only throttled at shared-medium-bucket granularity.

### MEDIUM [6]
- M1. `AccountRegister.password`, `AccountChangePassword.*_password`, `LostPassword.email`, `VerifyEmailRestricted.verify_token` are carried as `String` inside `Request` (no `RedactedString` wrapping).
- M2. `signals::install_handler` / `install_ignore` use `unsafe` blocks with no `// SAFETY:` comments (pcloud-daemon/src/signals.rs:283, 287, 290, 298, 300, 303).
- M3. The Unix-socket listener has no concurrency / connection-rate cap — serve loop is single-threaded but malicious peers can still monopolize the 5-second read window in tight reconnect loops (no accept-rate limiter).
- M4. `path_validation::validate_local_sync_path` does not cap path length; very long paths (PATH_MAX is platform-defined) are accepted and pushed down to OS syscalls that may truncate.
- M5. `DpapiVault::atomic_write` (Windows) does not set an explicit ACL or use `create_new`/`O_EXCL`-equivalent; relies on `fs::File::create` + `rename`. Per-user ACLs are inherited; NTFS inheritance may be relaxed.
- M6. `api_server` hints pulled from the server over `login` can rewrite the transport host/SNI without a final `validate()` pass — an attacker controlling the pCloud account response could redirect the SDK to a hostile IP after TLS handshake with the original name.

### LOW [3]
- L1. `PeerIdentity.pid` is synthesized as `0` on BSD/macOS (documented); audit logs that embed `pid` risk correlator pollution but `pid` is never used for authorization.
- L2. `auth.rs::current_effective_uid` uses `unsafe { libc::geteuid() }` with a `// SAFETY:` comment — good, but the `// SAFETY:` placement is ambiguous in Clippy terms (on a separate line above the block rather than inline, OK overall).
- L3. `CryptoShell::unwrap_active_dek` clones `wrapped_dek` (ciphertext) and `key_id` on every sector op — not a security issue but a minor heap-churn item that could surface under load.

## Detailed findings

### 1. Secret discipline

**Good baseline.** `pcloud-secret::SecretString` and `SecretBytes` implement all the hardening expected of an enterprise-grade secret type:
- `#[derive(ZeroizeOnDrop)]`, explicit `Zeroize` impls
  (pcloud-secret/src/secret_string.rs:35, 120; secret_bytes.rs:22, 98)
- No `Clone` derive; audit-visible `clone_secret()` helper
  (secret_string.rs:77-80)
- Constant-time `PartialEq` via `subtle::ConstantTimeEq`
  (secret_string.rs:110-112; secret_bytes.rs:91-93)
- Redacted `Debug` (`SecretString(<redacted>)`)
  (secret_string.rs:95-99)
- Deliberately no `Serialize`/`Deserialize` (secret_string.rs:126-127)

**H1 — auth tokens carried as `String` on the wire side of `pcloud-proto`**  
Severity: HIGH  
Files: `crates/pcloud-proto/src/methods/{auth,account,backup,folder,shares,notifications,public_links,crypto,diff,download}.rs` (dozens of `pub auth_token: String` / `pub password: String` struct fields). Representative citations:
- `crates/pcloud-proto/src/methods/auth.rs:173` `pub auth_token: String,`
- `crates/pcloud-proto/src/methods/auth.rs:217` `pub token: String,` (TFA)
- `crates/pcloud-proto/src/methods/auth.rs:273,314` (TFA send SMS / notification)
- `crates/pcloud-proto/src/methods/auth.rs:357` `pub digest_token: String,`
- `crates/pcloud-proto/src/methods/account.rs:255-259` `auth_token`, `current_password`, `new_password` all `String`
- `crates/pcloud-proto/src/methods/account.rs:323` `pub password: String,`
- `crates/pcloud-proto/src/methods/auth.rs:151-156` `password_params(&self, password: &str)` — borrows but still `&str`
- `crates/pcloud-proto/src/auth_api.rs:114` `auth_token: String`, 123 `challenge_token: String`
- `crates/pcloud-backends/src/account_backend.rs:324-325` `current_password: &str, new_password: &str`

Risk: these structs are long-lived in the binary-API pipeline; their `Debug` impl is auto-derived and will print `auth_token: "..."`. A stray `tracing::debug!(?req)` in the request path would leak the token. The daemon-side IPC envelopes are OK (they use `RedactedString`), but every method struct *after* the IPC boundary is back to plaintext `String`.

Remediation: introduce a single `pcloud_secret::TokenString` (already exists as `SecretString`) or a `Redacted<T: AsRef<str>>` wrapper and convert all `auth_token`/`password`/`token`/`digest_token` fields in `pcloud-proto/src/methods/*.rs` and `pcloud-proto/src/{auth,account,crypto}_api.rs`. Since these structs implement `Serialize` for the binary wire, the simplest low-risk step is a `RedactedString`-style newtype with `#[serde(transparent)]` and a redacted `Debug`.

**M1 — `Request` carries account-scope passwords as `String`**  
Severity: MEDIUM  
File: `crates/pcloud-ipc/src/methods.rs:951-980`. Variants `LostPassword.email`, `VerifyEmailRestricted.verify_token`, `AccountChangePassword.*_password`, `AccountRegister.password` use `RedactedString` for passwords (963-977) — OK.  
But on the daemon dispatch side (`runtime.rs:791-792, 2168-2169, 2229`), the values are **unwrapped into plain `String`** via `into_string()` before being passed into the backend. Until they are wrapped in `SecretString::new(...)` inside the handler, there is an intermediate `String` that is not zeroized on drop.

Remediation: change the handler signatures to accept `SecretString` and construct it at the destructuring site (`Request::AccountRegister { password, .. } => self.account_register(email, SecretString::new(password.into_string()), terms_accepted)`). Same for `unlock_crypto`, `setup_crypto`, `change_crypto_password`, `account_change_password`.

**H3 — public-link password destructured into `String`**  
Severity: HIGH (low data-criticality but same leakage pattern as passwords)  
File: `crates/pcloud-daemon/src/runtime.rs:622`:
```
Request::ChangePublicLinkPassword { link_id, password } => {
    self.change_public_link_password(link_id, password.map(|p| p.into_string()))
}
```
The `String` version is passed by value through `change_public_link_password` with no `SecretString` wrap. Public-link passwords are end-user chosen secrets and should not appear in `Debug` or be left unzeroized.

Remediation: route through `SecretString` end-to-end.

### 2. Auth vault

**Good file-vault posture** (`crates/pcloud-daemon/src/vault/file.rs`):
- Opt-in durable persistence: `auth_token` is the only secret persisted, gated by `VaultBackend::File` explicit config.
- File mode `0o600`, parent dir mode `0o700` (file.rs:138-185).
- Atomic write via `create_new` (O_CREAT|O_EXCL) + rename (file.rs:161-181). L3 regression guard is in place.
- Parent `fsync` after rename (file.rs:184, 238-243).
- Ownership validation via `symlink_metadata` + `uid` equality check (file.rs:199-220).
- Zeroizing intermediate buffers on load path (file.rs:88-132).
- `create_new` implicitly refuses symlinks; the test `store_token_refuses_to_follow_symlink_at_tmp_path` documents this.

**M5 — DpapiVault write is not hardened**  
Severity: MEDIUM (Windows-only path)  
File: `crates/pcloud-daemon/src/vault/dpapi.rs:91-103`:
- Uses `fs::File::create(&tmp)` (no `create_new`) and `fs::rename`.
- Does not `sync_all` the parent directory.
- No explicit NTFS ACL — relies on inherited ACLs from the parent `config` directory.

Remediation: (a) switch `fs::File::create` → `OpenOptionsExt::custom_flags(CREATE_NEW)` or `.create_new(true)`; (b) call `fs::File::open(parent).sync_all()` after rename; (c) apply an explicit restrictive DACL to the parent dir at init time so the DPAPI ciphertext inherits owner-only access.

### 3. Local IPC

**Good baseline:**
- Socket file mode `0o600` under `0o700` parent (ipc/transport.rs:246-267).
- Peer-credential check on every accept: `SO_PEERCRED` (linux.rs:31-57), `getpeereid(3)` (unix.rs:44-60), and `GetNamedPipeClientProcessId` + TokenUser SID compare (platform/windows.rs). Unauthorized peers receive `ResponseStatus::Unauthorized` (transport.rs:186-208).
- Frame length cap: `MAX_REQUEST_BYTES = 1 MiB`, enforced BEFORE allocation proportional to the attacker-controlled `u32 payload_len` prefix (server.rs:42; transport.rs:304-325).
- Oversize frames: connection closed without reply to avoid amplification (transport.rs:337-340).
- 5-second read timeout on each stream (transport.rs:32, 184). Slow-client isolation tested (transport.rs:543-597).
- Version negotiation via `u16 version` header, `VersionMismatch` rejected early (protocol.rs:252-260).
- Malformed-request test confirms that a bad peer does not affect follow-up requests (transport.rs:599-666).

**M3 — no accept-rate / connection-rate limiter**  
Severity: MEDIUM  
Files: `crates/pcloud-daemon/src/serve.rs`, `crates/pcloud-ipc/src/transport.rs`.  
The serve loop is single-threaded (one request per iteration), but an attacker running as the same uid (within the per-user trust boundary) can connect, send a malformed frame that triggers a 5s read timeout, disconnect, and repeat. There is no per-source accept-rate limit or global in-flight cap. The owner-only chmod/peer-UID check means only the daemon-owning user can do this, which reduces severity to a local same-user DoS.

Remediation: add a simple token-bucket around `bound.serve_once()` calls with a cap like 50 connections/second, logging and dropping excess accepts.

### 4. Outbound transport policy

**H2 — `total_request_timeout` is dead code in the synchronous path**  
Severity: HIGH (defence-in-depth regression)  
File: `crates/pcloud-proto/src/transport.rs`.
- `TransportConfig::total_request_timeout` is declared at line 99 and documented (lines 93-99) as "outer deadline shared across all stages".
- `send_and_receive` (344-370) threads `timeout` through the write / flush / read helpers, but it is set to `config.read_timeout` (`execute_tls` line 341) or a hard-coded 15 s (`execute_plain` line 321). The outer `total_request_timeout` is never consulted.
- Result: a stuck server that repeatedly causes `WouldBlock` on each stage (write, flush, read) can keep a worker thread pinned indefinitely because each stage resets the deadline.

Remediation: inside `execute_with_body` (262-273) compute `let deadline = Instant::now() + config.total_request_timeout;` and pass it (not a `Duration`) into each helper; each helper should early-return on `Instant::now() >= deadline` regardless of the per-stage timeout.

**Good baseline for TLS enforcement:**
- `ApiEndpoint::validate` rejects `Production + Plaintext` centrally (api.rs:131-141, tests 260-267).
- `ApiMode::secure_default_for(Production)` yields `Tls` (api.rs:210-215).
- `TransportConfig::use_tls` defaults to `true` in the documented constructor examples; the field is public only so tests can set `false`.
- No `http://` URL allowed in production profiles except the schema URL (config/schema.rs:24 is `http://json-schema.org/...` which is a namespace, not a network endpoint) and `config/file_history.rs:68` which is explicitly gated on non-production profiles.
- `webpki-roots` + rustls with no "accept any cert" switch (transport.rs:330-342).

**M6 — server-supplied `apiserver` hints bypass `validate()`**  
Severity: MEDIUM  
Files: `crates/pcloud-proto/src/transport.rs:276-293`, `crates/pcloud-config/src/api.rs:178-189`.
- `apply_api_server_hint` rewrites `host` AND `server_name` unconditionally.
- If a compromised server returns `apiserver: "evil.example.com:443"`, subsequent handshakes verify against `evil.example.com`, which the attacker trivially passes with a cert for that name.
- `is_known_safe_host` exists (api.rs:224-231) but is only documented as "advisory" and is never actually called from `apply_api_server_hint`.

Remediation: (a) in `BinaryApiTransport::apply_api_server_hint`, reject hints whose host is not `ends_with(".pcloud.com")` / `.pcloud.link` unless an operator config opt-in is set; (b) re-run `ApiEndpoint::validate(environment)` after applying the hint; (c) log an audit event on every applied hint so ops can spot unexpected rewrites.

### 5. Crypto

**Good baseline** (`crates/pcloud-crypto/src/lib.rs`, `content.rs`, `keys.rs`):
- `#![forbid(unsafe_code)]` at the crate root (lib.rs:1).
- Master key material in `SecretBytes` with zeroize-on-drop (lib.rs:7-19).
- Constant-time fingerprint comparison `subtle::ConstantTimeEq` (keys.rs:19, 200-205; lib.rs:936-943 for constant-time old-vs-new password compare in `change_password`).
- Fresh 12-byte random nonce from `getrandom` per sector encrypt (content.rs:188-190).
- AEAD: AES-256-GCM with sector index bound into AAD (content.rs:191-207, 249-269).
- Locked-shell check before any key derivation (lib.rs:1149-1170).
- KMS-wrapped DEK path evicts the process-local cache on `stop()` (lib.rs:754-767) so plaintext DEK does not outlive the session.
- Password-change refuses identical passwords via constant-time compare (lib.rs:934-943).
- Argon2id default parameters in `KeyManager::derive_key_material` (keys.rs) — reviewed; uses salt.
- `CryptoError` variants are opaque: no plaintext/ciphertext/nonce bytes in any error string (lib.rs:117-185).

**H4 — No throttle for wrong-password attempts**  
Severity: HIGH  
Files: `crates/pcloud-daemon/src/{rate_limit,runtime}.rs`.
- Dispatcher rate-limit categorises `UnlockCrypto` (→ `Request::CryptoUnlock`) and `SubmitPassword` (→ `Request::PasswordSubmission`) as `RateCategory::Medium` (rate_limit.rs:173-209). There is no dedicated "auth attempt" bucket.
- `CryptoShell::start` returns `CryptoError::WrongPassword` after running Argon2id against the stored fingerprint (crypto/lib.rs:713-738). Argon2id itself provides the cost; but because the dispatcher does not observe wrong-password outcomes, an attacker can burn through a dictionary as fast as the Medium bucket allows (default refill ~a few per second).
- Same-user-only (IPC is owner-gated) somewhat mitigates this, but the same-user threat model must include a compromised child process attempting to escalate into crypto material.

Remediation: add a separate `AuthAttempt` rate category with a slow-refill bucket (e.g. 5 tokens/min with 10 capacity) applied to `Request::PasswordSubmission`, `Request::TwoFactorCodeSubmission`, `Request::CryptoUnlock`, `Request::CryptoSetup` (for first-time setup brute force on an empty fingerprint is irrelevant but future fingerprint-verify paths should share), `Request::CryptoChangePassword`. On rejection, return `ResponseStatus::Unauthorized` with "too many failed attempts, retry after Ns" rather than `Conflict`, so callers can distinguish.

### 6. Memory safety (`unsafe` blocks)

**Good posture for most unsafe call-sites**  
Every `unsafe { libc::... }` in the IPC transport has a `// SAFETY:` comment:
- `ipc/transport.rs:139-151` (setsockopt) — no SAFETY comment on the block directly; the wrapping comment is terse. Minor: add inline `// SAFETY:`.
- `ipc/platform/linux.rs:42, 69, 105` — all have `// SAFETY:` comments.
- `ipc/platform/unix.rs:49-52` — has SAFETY comment.
- `ipc/auth.rs:66-68` — has SAFETY comment.
- `daemon/vault/dpapi.rs:64-68, 75-87, 122-137, 161-184` — all annotated.
- `daemon/mount_runtime.rs:1177, 1183` — annotated.

**M2 — signals.rs missing SAFETY comments**  
Severity: MEDIUM  
File: `crates/pcloud-daemon/src/signals.rs:283, 287, 290, 298, 300, 303`.
Six `unsafe { ... }` blocks with no `// SAFETY:` comment. The unsafe operations are `std::mem::zeroed::<libc::sigaction>()`, `libc::sigemptyset(&mut sa.sa_mask)`, and `libc::sigaction(sig, &sa, std::ptr::null_mut())`. All are well-known idioms and sound, but the project invariant (per `lib.rs` — `#![warn(unsafe_op_in_unsafe_fn)]`) calls for SAFETY comments at every unsafe site.

Remediation: add `// SAFETY:` comments explaining:
- `std::mem::zeroed::<libc::sigaction>()` — sigaction is a C POD; all-zero is a valid initialized state for the subset of fields we overwrite before the `sigaction` syscall.
- `libc::sigemptyset(&mut sa.sa_mask)` — `sa.sa_mask` is a valid writable `sigset_t` location; POSIX contract.
- `libc::sigaction` — `sig` is a valid signal number, `&sa` is a fully-initialized `sigaction`, `null_mut()` for the `oldact` out-arg is documented as "don't want it".

### 7. Input validation

**Good baseline** (`crates/pcloud-ipc/src/path_validation.rs`):
- Rejects non-UTF-8 (line 47).
- Rejects embedded NUL bytes (52-54).
- Rejects `..` components (59-63).
- Rejects root-level symlinks to prevent TOCTOU (70-75).

**M4 — no maximum path length enforced**  
Severity: MEDIUM  
File: `crates/pcloud-ipc/src/path_validation.rs`.
The validator is silent on total path length. Very long paths (beyond `PATH_MAX = 4096` on Linux) are rejected later by the kernel, but an IPC client can submit a 1-MiB-minus-8-bytes path (just under `MAX_REQUEST_BYTES`) that survives the validator and burns CPU/IO downstream.

Remediation: cap at something reasonable for sync roots (e.g. `4096 * 4 = 16 KiB` total and `255` per component) and return `PathValidationError::TooLong`.

### 8. Logging discipline

**Good posture overall.**  
Grepping `log::(info|warn|error|debug)|tracing::...` across the workspace shows no call site that logs a raw secret. The one match for `log::info!(...token...)` at `crates/pcloud-daemon/src/serve.rs:309` logs only the string `"token refreshed successfully"` with no payload.

`pcloud-secret/src/lib.rs` documentation explicitly asserts: _"never reach a formatter, so accidental `tracing::debug!(?token)`"_ (lib.rs:24). The `RedactedString::Debug` impl in `pcloud-ipc/src/redacted.rs:75-79` prints `<redacted N bytes>`.

No findings in this category.

### 9. DoS mitigations

**Good baseline:**
- Per-request payload cap (1 MiB, server.rs:42).
- Per-read timeout (5 s, transport.rs:32).
- Session-per-peer rate limiter with category-aware buckets (rate_limit.rs; Medium and Expensive categories with token-bucket semantics).
- Graceful drain / quiesce (serve.rs:134-178).
- Bootstrap rejects `Plaintext` in Production (api.rs:137-141).

**Gaps already listed above:** M3 (accept-rate limit), H4 (auth-attempt-specific bucket), H2 (total request deadline).

## Remediation priority summary

1. **H2** — wire `total_request_timeout` into the outbound deadline loop. One-file, high-impact.
2. **H4** — add `AuthAttempt` rate category and apply to password/TFA/crypto-unlock requests.
3. **H1** — migrate `pcloud-proto` method struct fields (`auth_token`, `password`, `token`, `digest_token`) to a `RedactedString`/`SecretString`-backed wrapper.
4. **H3** — route `ChangePublicLinkPassword` through `SecretString` on the daemon side.
5. **M1** — daemon handlers should take `SecretString`, not `String`.
6. **M6** — validate / allow-list `apiserver` hints before mutating the transport.
7. **M2** — add SAFETY comments in `signals.rs`.
8. **M3** — accept-rate limiter around `serve_once`.
9. **M4** — cap path length in `path_validation`.
10. **M5** — harden `DpapiVault::atomic_write`.

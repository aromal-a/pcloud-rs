# Section 2 — Security Audit (Round 03)

**Audit date:** 2026-04-17
**Auditor:** senior Rust security engineer (read-only)
**Scope:** secret discipline, auth vault, IPC, rate limiting, transport,
pcloud-web, unsafe blocks, input validation, logging
**Mode:** fresh audit after prior 01/02 rounds; verify claimed fixes and
surface remaining gaps

## Executive summary

Round-03 findings are mixed. Several key fixes from round-02 **did** land:

- `RedactedProtoString` exists at
  `crates/pcloud-proto/src/redacted.rs:40-127` with correct serde-transparent
  serialisation and redacted `Debug`/`Display`.
- All request-struct `auth_token`, `password`, and TFA `code` fields across
  `crates/pcloud-proto/src/methods/*.rs` are now typed as
  `RedactedProtoString` (~50 call sites verified).
- Auth-vault file implementation at
  `crates/pcloud-daemon/src/vault/file.rs:77-196` is strong (O_EXCL + mode
  0600, parent 0700, symlink-metadata check, zeroize on load-error paths).
- IPC transport at `crates/pcloud-ipc/src/transport.rs` correctly enforces
  owner-only socket, peer uid check, 5 s read timeout, and MAX_REQUEST_BYTES.
- Session-token gate at `crates/pcloud-web/src/routes.rs:651-687` is
  applied to every mutating route with a hand-rolled constant-time compare.

However, **several round-02 remediations were not landed**:

- **H4 (rate limit) — NOT FIXED.** No `AuthAttempt` category exists in
  `crates/pcloud-config/src/rate_limit.rs` or
  `crates/pcloud-daemon/src/rate_limit.rs`.
- **H2 (total_request_timeout) — NOT FIXED.** Neither
  `crates/pcloud-proto/src/transport.rs` nor the config crate exposes a
  `total_request_timeout`; `execute_plain` still hard-codes 15 s and
  `execute_tls` still reuses `read_timeout` as the "deadline".
- **M6 (api_server hint hardening) — NOT FIXED.** No `is_known_safe_host`
  helper exists anywhere. `apply_api_server_hint` in both
  `pcloud-config/src/api.rs:178` and `pcloud-proto/src/transport.rs:276`
  accepts arbitrary operator/server-supplied host strings without validation.
- **Privileged-request audit logging in `serve.rs` — NOT ADDED.** `grep`
  found no `audit_log`, `privileged`, `classify_privileged` construct on
  the dispatch hot path.
- **`MAX_IPC_CONNECTIONS` cap — NOT ADDED.** No constant, no accept-gate.
- **SAFETY comments on `signals.rs` — NOT ADDED.** Three `unsafe` blocks
  (lines 283-290, 298-303, 287-289) remain with zero `// SAFETY:` comments.

Plus new findings:

- Plaintext-String window during crypto-password dispatch in `runtime.rs`.
- `RedactedString`/`RedactedProtoString` do not zeroize on drop.
- Web auth token is written to stderr (`eprintln!`) at startup —
  captured by journald / log aggregation.
- `write_timeout` is never set on IPC response streams.
- No connection-acceptance cap (DoS).

---

## Findings

### CRITICAL

_None._ The round-02 critical items (cleartext password persistence,
plaintext production transport, world-readable socket) remain remediated.

---

### HIGH

#### H-1. `total_request_timeout` still not enforced — outer deadline missing

- `crates/pcloud-proto/src/transport.rs:266-273` selects between
  `execute_plain` and `execute_tls`.
- `execute_plain` at line 322 passes `Duration::from_secs(15)` — a
  hard-coded constant, not operator-controlled.
- `execute_tls` at line 342 passes `config.read_timeout` as the loop
  "deadline". `read_timeout` is the **per-syscall** timeout
  (`TransportConfig` lines 93-95), not an outer request-wide deadline.
- Consequence: a server that responds with a trickle of bytes just under
  the per-read timeout can keep a worker busy indefinitely. The prior
  audit flagged this; the fix has not landed. There is no field or
  config key named `total_request_timeout` anywhere in the workspace.

Remediation: add `TransportConfig::total_request_timeout: Duration`,
compute `let deadline = Instant::now() + total_request_timeout` at the
top of `execute_with_body`, and abort any loop iteration whose
`Instant::now() >= deadline`. Remove the hard-coded 15 s.

#### H-2. No `AuthAttempt` rate-limit category — brute-force gate absent

- `crates/pcloud-daemon/src/rate_limit.rs:173-211` classifies requests
  into `Cheap`/`Medium`/`Expensive` only.
- `Request::PasswordSubmission`, `Request::AuthTokenSubmission`,
  `Request::TwoFactorCodeSubmission`, `Request::CryptoUnlock`, and
  `Request::CryptoChangePassword` all fall into the default `Medium`
  bucket (30 req/min default).
- Consequence: a hostile local client (post-peer-uid-check by-pass, or
  a compromised process running as the daemon user) has 30 login
  attempts per minute per session — insufficient hardening against
  guessing.
- The prior-audit remediation (`AuthAttempt` category, 5-10
  req/min, per-session bucket on password/TFA/crypto-unlock)
  is not present. Confirmed: `grep -r "AuthAttempt" crates/` returns
  nothing.

Remediation: add `RateCategory::AuthAttempt` in
`crates/pcloud-config/src/rate_limit.rs:41`, plumb a distinct bucket
(default 5 tok capacity, 0.1 tok/s refill) in `SessionRateLimiter::new`,
and map the affected request variants in `rate_limit::categorize`.

#### H-3. `api_server` server-hint accepted without host-allowlist validation

- `crates/pcloud-config/src/api.rs:178-189` and
  `crates/pcloud-proto/src/transport.rs:276-293` both accept the
  server-supplied `apiserver` hint and rewrite `host` / `server_name`
  verbatim.
- No allowlist, no TLD suffix check, no `is_known_safe_host` helper
  (none exists anywhere in the tree).
- Residual defence: TLS cert validation still rejects a wrong CN/SAN.
  But under `ApiMode::Plaintext` (test profile, or any misconfiguration
  that disables TLS) a rogue server response can silently retarget
  every subsequent request. Under TLS, a misconfigured `server_name`
  field (no validation against an allowlist) plus a rogue upstream
  that controls certificate issuance for attacker-owned names would
  succeed.

Remediation: introduce `pcloud_config::api::is_known_safe_host(host: &str)
-> bool` that checks suffix against `[".pcloud.com", ".pcloud.cloud"]`
and call it from both `apply_api_server_hint` implementations. Reject
with a diagnostic log entry and leave the endpoint unchanged on
mismatch.

#### H-4. Privileged-request audit logging not implemented in serve.rs

- Round-02 remediation asked for an audit-log emission at dispatch when
  a privileged request (`Shutdown`, `CryptoChangePassword*`,
  `AuditVerifyChain`, mount control, etc.) is served.
- `grep -n "audit\|privileged\|classify_privileged"
  crates/pcloud-daemon/src/serve.rs` returns only the idle-logout
  persistence at `serve.rs:312-319`, which is the session-refresh
  hook, not the dispatch-path audit.
- Consequence: no tamper-evident trace of privileged actions; if a
  local attacker drops into the uid-owning process they can issue
  `Shutdown`/`CryptoChangePassword` without leaving an audit row.

Remediation: wrap `dispatch_with_drain_gate` so every method in a
`PRIVILEGED_METHODS` set triggers `pcloud_store::append_audit_event(...)`
with peer uid, pid, method name, and timestamp. The audit store already
exists and is used in `serve.rs:313`.

---

### MEDIUM

#### M-1. Plaintext `String` window for crypto passwords at dispatch boundary

- `crates/pcloud-daemon/src/runtime.rs:567,569,578-579,590-594` all call
  `.into_string()` on a `RedactedString` (IPC wire type) to get a bare
  `String` and pass it through as a `String` argument. The callees
  (`unlock_crypto`, `setup_crypto`, `change_crypto_password`,
  `change_crypto_password_unlocked`) then construct a `SecretString`
  internally (see e.g. runtime.rs:2543, 2578, 2740-2741).
- Consequence: between `.into_string()` and the later `SecretString::new`,
  the bare `String` sits on the stack **without zeroize-on-drop**. If
  the process crashes or the memory is swapped, the plaintext
  passphrase is recoverable.
- `RedactedString` itself (`crates/pcloud-ipc/src/redacted.rs`) and
  `RedactedProtoString` (`crates/pcloud-proto/src/redacted.rs`) do
  **not** derive `ZeroizeOnDrop` — they only redact `Debug`.

Remediation: change the crypto-password helper signatures to accept
`SecretString` directly; wrap the password in a `SecretString` at the
dispatch site (`let pw = SecretString::new(password.into_string()); ...`)
instead of carrying it as `String`. Consider making
`RedactedString::into_secret()` the idiomatic consumer that yields a
`SecretString` in one shot.

#### M-2. IPC response path has no write timeout

- `crates/pcloud-ipc/src/transport.rs:184` sets `set_read_timeout` but
  never sets `set_write_timeout`. If a peer reads slowly from the
  response half, the writer blocks indefinitely.
- `crates/pcloud-proto/src/transport.rs:311` does set `set_write_timeout`
  for outbound API calls — good.

Remediation: in `serve_stream_once` call
`stream.set_write_timeout(Some(Duration::from_secs(5)))?` after the
`set_read_timeout` line.

#### M-3. No `MAX_IPC_CONNECTIONS` acceptance cap

- `BoundIpcServer::serve_once` accepts one connection per call, but
  there is no ceiling on the number of concurrent in-flight dispatchers.
- `grep -n "MAX_IPC_CONNECTIONS\|max_connections\|connection_limit"
  crates/pcloud-ipc/ crates/pcloud-daemon/` returns zero matches in
  the IPC path.
- Consequence: a malicious local peer can open N connections while the
  serve loop is single-threaded — harmless today — but a future
  multi-thread refactor would have no ceiling on fd consumption.

Remediation: add `const MAX_IPC_CONNECTIONS: usize = 64;` and an
`AtomicUsize` counter that gates `serve_once`, rejecting overflow with
`ResponseStatus::Unavailable`.

#### M-4. No SAFETY comments on `signals.rs` unsafe blocks

- `crates/pcloud-daemon/src/signals.rs:283,287-289,290,298,301,303`
  contain `unsafe` calls into libc (`std::mem::zeroed`, `sigemptyset`,
  `sigaction`) with no `// SAFETY:` prefix.
- `grep "SAFETY" crates/pcloud-daemon/src/signals.rs` returns nothing.
- `crates/pcloud-ipc/src/auth.rs:67` (`libc::geteuid`) is also
  unannotated.
- The SAFETY invariants ARE briefly discussed in the file-level doc
  comment (lines 10-21), but per-block `// SAFETY:` is what audit
  tooling (e.g. `clippy::undocumented_unsafe_blocks`) expects.

Remediation: add per-block `// SAFETY:` comments justifying
initialisation, pointer validity, and signal-safety.

#### M-5. Web auth token leaked to stderr at startup

- `crates/pcloud-web/src/lib.rs:279`:
  `eprintln!("[pcloud-web] auth token: {}", config.web_token);`
- stderr is typically captured by journald, log aggregation, ephemeral
  log files, systemd's `StandardError=`, and shell redirection
  (`pcloud-daemon 2>&1 | tee log.txt`).
- Consequence: the session token lives in every operator log forever,
  accessible to anyone with log-read privileges.

Remediation: emit the token **once** to a mode-0600 file such as
`<runtime_dir>/web_token` and require the operator to `cat` it
explicitly. Alternatively, emit only the first/last 4 chars to the log
and persist the full token to the secure file.

#### M-6. `RedactedString`/`RedactedProtoString` do not zeroize

- `crates/pcloud-ipc/src/redacted.rs:39` and
  `crates/pcloud-proto/src/redacted.rs:42` are `struct X(String)` with
  `#[serde(transparent)]` but no `ZeroizeOnDrop`.
- Because IPC `Request` values travel through deserialisation in the
  serve loop and are then destructured, the backing `String` allocation
  is dropped without scrubbing between the deserialise and the
  `SecretString::new(...)` re-wrap (see M-1).

Remediation: add `impl Drop for RedactedString` (and the proto variant)
that calls `self.0.zeroize()`, plus derive `ZeroizeOnDrop` from the
`zeroize` crate (non-breaking, the wire format is already `String`).

#### M-7. TOCTOU on `path_validation.rs` symlink check

- `crates/pcloud-ipc/src/path_validation.rs:88-93`: `path.exists()`
  followed by `symlink_metadata` is a textbook TOCTOU — an attacker
  can win the race between `validate` and the caller's later
  `canonicalize`.
- Acknowledged implicitly in the module doc (lines 17-25), but the
  canonicalize step is **still outside the validator**.

Remediation: fold the canonicalize step into the validator so the
returned path is the one the caller uses; or emit a `Dir`-handle via
`openat2(RESOLVE_NO_SYMLINKS)` on Linux (requires `rustix`).

---

### LOW

#### L-1. Hand-rolled constant-time compare in pcloud-web

- `crates/pcloud-web/src/routes.rs:626-635` and
  `crates/pcloud-web/src/routes.rs:666-675` implement constant-time
  equality by XOR-folding bytes.
- The code is correct today, but `subtle::ConstantTimeEq::ct_eq`
  (already a dependency of `pcloud-secret`) is the canonical
  implementation and harder to regress.

Remediation: depend on `subtle` in `pcloud-web/Cargo.toml` and replace
both hand-rolled folds with `a.ct_eq(b).into()`.

#### L-2. Crypto `code: String` in `ChangeUserPrivateRequest`

- `crates/pcloud-proto/src/methods/crypto.rs:44`: the email-flow
  confirmation code is `String`, not `RedactedProtoString`.
- It is short-lived and non-catastrophic if logged, but for uniform
  discipline it should also be `RedactedProtoString`.

Remediation: change the field type; no wire-format impact (serde
transparent).

#### L-3. `parse_api_server_hint` accepts any `u16` port including 0

- `crates/pcloud-proto/src/transport.rs:466-474` and
  `crates/pcloud-config/src/api.rs:205-213`: no rejection of port 0
  or reserved ports. `validate` catches port 0 on the endpoint
  struct, but a mid-session hint can race past if the validator is
  not re-invoked after `apply_api_server_hint`.

Remediation: short-circuit when `port == 0` inside the parser.

#### L-4. Test code in `methods/mod.rs` uses plaintext literals

- `crates/pcloud-proto/src/methods/mod.rs:170-188` constructs a
  `ChangePasswordRequest` with literals `"old"` / `"new"` etc.
- Tests are not production, but the `register.password = "strong"`
  pattern is copy-pasted elsewhere in examples; if those ever
  graduate to docs, they should use `RedactedProtoString::new(...)`.

---

## Verification matrix — round-02 claimed fixes vs round-03 reality

| # | Claim | Status | Evidence |
|---|---|---|---|
| 1 | RedactedProtoString created | LANDED | `pcloud-proto/src/redacted.rs:1-185` |
| 2 | `auth_token`/`password` fields typed as RedactedProtoString | LANDED | ~50 sites across `methods/*.rs` |
| 3 | Vault mode 0600 / parent 0700 / O_EXCL / ownership check | LANDED | `vault/file.rs:138-186, 198-221` |
| 4 | Vault zeroize on load | LANDED | `vault/file.rs:88-132` |
| 5 | IPC owner-only socket + peer cred check | LANDED | `transport.rs:246-267, 186-208` |
| 6 | IPC frame cap + 5 s read timeout + slow-client isolation | LANDED | `transport.rs:32, 184, 304-325` |
| 7 | IPC `MAX_IPC_CONNECTIONS` cap | NOT LANDED | no constant found |
| 8 | IPC write timeout on response | NOT LANDED | no `set_write_timeout` in ipc/transport.rs |
| 9 | `AuthAttempt` rate-limit category | NOT LANDED | no match in `rate_limit.rs` either crate |
| 10 | `total_request_timeout` enforced end-to-end | NOT LANDED | field does not exist anywhere |
| 11 | TLS-only in production | LANDED | `api.rs:137-141` |
| 12 | `is_known_safe_host` integration for api_server hint | NOT LANDED | helper does not exist |
| 13 | pcloud-web session-token gate | LANDED | `routes.rs:659-687`, 4 call sites |
| 14 | pcloud-web constant-time compare | LANDED (hand-rolled) | `routes.rs:666-674` |
| 15 | pcloud-web mutating routes gated | LANDED | 4 `require_web_token` calls |
| 16 | SAFETY comments on signals.rs unsafe blocks | NOT LANDED | 0 matches in file |
| 17 | DPAPI vault SAFETY comments | LANDED | `vault/dpapi.rs:65-68, 75-78, 122-126, 161-165` |
| 18 | `ipc/platform/*` SAFETY comments | LANDED | 25 matches across 3 files |
| 19 | SecretString wrapping at crypto dispatch site | PARTIAL | wrapping happens inside helpers, not at dispatch (M-1) |
| 20 | Privileged-request audit logging in serve.rs | NOT LANDED | no construct present |

**6 of 20 claimed remediations are not landed** (rows 7, 8, 9, 10, 12, 16, 20; with row 19 partial).

---

## Priority for Round-04

1. **H-1 (total_request_timeout)** — simple, single-file transport fix;
   highest impact against slow-loris style DoS.
2. **H-2 (AuthAttempt bucket)** — add one enum variant + one bucket;
   blocks local brute-force.
3. **H-3 (api_server host allowlist)** — `is_known_safe_host` + two
   call sites; closes silent-retarget vector.
4. **H-4 (privileged audit logging)** — attach to dispatch path; makes
   the existing audit store actually useful for forensics.
5. **M-1 (SecretString at dispatch)** — refactor four dispatch arms in
   runtime.rs.
6. **M-4 / M-6 / M-2 / M-3** — hygiene: SAFETY comments, zeroize the
   redactors, write timeout, connection cap.

---

## Files consulted

- `crates/pcloud-secret/src/secret_string.rs`
- `crates/pcloud-secret/src/secret_bytes.rs`
- `crates/pcloud-proto/src/redacted.rs`
- `crates/pcloud-proto/src/methods/*.rs` (grep survey)
- `crates/pcloud-proto/src/methods/crypto.rs`
- `crates/pcloud-proto/src/methods/public_links.rs`
- `crates/pcloud-proto/src/methods/mod.rs`
- `crates/pcloud-proto/src/transport.rs`
- `crates/pcloud-ipc/src/redacted.rs`
- `crates/pcloud-ipc/src/transport.rs`
- `crates/pcloud-ipc/src/path_validation.rs`
- `crates/pcloud-daemon/src/auth_vault.rs`
- `crates/pcloud-daemon/src/vault/file.rs`
- `crates/pcloud-daemon/src/vault/dpapi.rs`
- `crates/pcloud-daemon/src/rate_limit.rs`
- `crates/pcloud-daemon/src/signals.rs`
- `crates/pcloud-daemon/src/serve.rs`
- `crates/pcloud-daemon/src/runtime.rs` (selected sites)
- `crates/pcloud-config/src/api.rs`
- `crates/pcloud-config/src/rate_limit.rs`
- `crates/pcloud-web/src/lib.rs`
- `crates/pcloud-web/src/routes.rs`

No source files were modified during this audit.

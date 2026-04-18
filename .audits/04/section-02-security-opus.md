# Section 2: Security — Audit 04 (Opus)

**Date:** 2026-04-18
**Scope:** secret discipline (SecretString/SecretBytes, redacted Debug, zeroize),
auth vault hygiene, local IPC peer-cred/mode checks, production TLS
enforcement, sensitive-data exposure paths.

## Finding counts

- CRITICAL: 0
- HIGH: 2
- MEDIUM: 4
- LOW: 5
- **Total: 11**

Overall posture is strong. Secret wrappers, vault hardening, IPC owner-only
peer-cred auth, and production TLS refusal are implemented correctly and
widely adopted. Residual findings are boundary leaks and missing
zeroize-on-drop for a small number of transient plaintext holders.

---

## HIGH

### H1 — Plaintext `String` passwords held across runtime method bodies
`crates/pcloud-daemon/src/runtime.rs:2199-2200` (`current_password`,
`new_password`), `:2260` (`account_register.password`), `:2739`
(`unlock_crypto.password`), `:2774` (`setup_crypto.password`), `:2909-2912`
(`change_crypto_password`), `:2966` (`new_password`).
These methods accept bare `String` rather than `SecretString`. The wrapper
is constructed inside the body (e.g. `runtime.rs:2746`, `:2781`), meaning
the inbound parameter-`String` is not `ZeroizeOnDrop`; it lingers on the
caller's stack/IPC decode buffer after move and is not scrubbed. Convert
the method signatures to `SecretString` (or `RedactedString` → immediate
`SecretString`) so the whole in-memory lifetime is wrapped. The dispatcher
already owns a decoded request; plumbing `SecretString` from
`ipc::methods` through to these entry points closes the gap.

### H2 — Web session token stored as plaintext `String`, not a secret wrapper
`crates/pcloud-web/src/lib.rs:175` (`WebConfig.web_token: String`), `:245`
(`AppState.web_token: Arc<String>`), `:264`
(`write_web_token_to_runtime_dir(token: &str)`), `:190`
(`generate_web_token()` returns `String`).
The 256-bit hex session bearer gates every mutating route
(`routes.rs:688`). Constant-time compare is correct (`routes.rs:698`),
but the token itself is held in plain `String`/`Arc<String>` for the
daemon lifetime with no zeroize-on-drop and a non-redacted `Debug` impl
on `WebConfig` (derived via `#[derive(Debug, Clone)]`, `lib.rs:160`).
Any `tracing::debug!(?config)` would render the token verbatim. Wrap in
`SecretString`, remove derived `Debug`, and hand-write a redacted impl.

---

## MEDIUM

### M1 — `verify_token` / `auth_token` / `id_token` as bare `&str`/`String`
`crates/pcloud-backends/src/crypto_backend.rs:237,248` (`auth_token: &str`),
`crates/pcloud-backends/src/account_backend.rs:305` (`verify_token: &str`),
`crates/pcloud-ipc/src/methods.rs:955` (`verify_token: String`),
`crates/pcloud-idp/src/exchange.rs:209`, `crates/pcloud-idp/src/jwks.rs:196`,
`crates/pcloud-idp/src/oidc.rs:148` (`id_token: String`),
`crates/pcloud-fleet/src/lib.rs:229` (`token: String`).
These are credential-equivalent bearer values flowing through APIs with
no `SecretString` wrapping and no redaction at the serde boundary.
`id_token` especially is a full JWT whose contents and signature suffice
to impersonate the user with the IdP exchanger. Promote to
`SecretString`/`RedactedString` on the wire and at the call site.

### M2 — Auth vault error surface leaks no content, but `load_token` still
copies into an intermediate `Vec<u8>` that is zeroized only on the happy
path and some error paths. `crates/pcloud-daemon/src/vault/file.rs:118`
(`trimmed_bytes = buf[start..end].to_vec()`). If `String::from_utf8` fails
the invalid bytes are scrubbed (`:127`), but the `trimmed_bytes` copy is
moved into `from_utf8` and only zeroized on the `Err` branch; on panic
between `:118` and `:121` the slice is not wrapped. A `scopeguard` or
immediate wrap-then-parse pattern would remove the gap.

### M3 — `RedactedString` is the IPC boundary redactor but does not zeroize.
`crates/pcloud-ipc/src/redacted.rs:39` (`struct RedactedString(String)`).
`Debug` is redacted and serde works, but there is no `Drop`/`ZeroizeOnDrop`.
Every decoded IPC password/2FA code lives in one of these across the
dispatcher path before it becomes a `SecretString`. Add
`#[derive(ZeroizeOnDrop)]` on the inner `String` (or convert the field to
`SecretString` internally while keeping the serde-visible wrapper).

### M4 — No `SECCOMP` / landlock sandbox on the daemon.
`crates/pcloud-daemon/src/bootstrap.rs` and `main.rs` contain no syscall
or path restriction even though the daemon handles decrypted crypto
material and persistent tokens. At minimum Linux `landlock` to restrict
filesystem writes to the sync-root + runtime dir, and seccomp-bpf to
block `ptrace`/`process_vm_readv`, would harden against a same-UID
attacker (the IPC peer-cred check only authorizes; it does not
compartmentalize).

---

## LOW

### L1 — `Default` impl for `WebConfig` generates a token via `getrandom`
that panics on failure. `crates/pcloud-web/src/lib.rs:124`
(`getrandom::getrandom(&mut buf).expect("kernel RNG unavailable")`).
Panic on a security primitive is OK for startup, but a typed error at
construction is preferable so supervisors can surface the condition
rather than crashloop.

### L2 — `pcloud-secret/examples/roundtrip.rs:24` prints `Debug` of a
`SecretString`. Output is redacted by design, but as a public example it
invites copy/paste in non-wrapper code paths; add a comment and move to
integration tests.

### L3 — `store_token` on Windows does not translate 0700/0600 to an NTFS
DACL. `crates/pcloud-daemon/src/vault/file.rs:143-146` documents the
intent and steers users to `DpapiVault`, but when `PCLOUD_VAULT=file` is
set explicitly on Windows the token is written with default ACLs. Either
refuse `VaultBackend::File` on Windows or apply a DACL equivalent to the
DPAPI path.

### L4 — `pcloud-mockserver/src/lib.rs:84`: `TEST_TOKEN` is a const string.
Not a runtime secret, but grepping the tree may yield it alongside real
`pcloud-live-e2e` env names — worth clearly marking `#[cfg(test)]`-only
exposure.

### L5 — `sid_to_string` on Windows IPC uses `LocalAlloc`'d UTF-16 and copies
into a `String` without scrubbing (`crates/pcloud-ipc/src/platform/windows.rs:365`).
SIDs are public identifiers, not secrets, so severity is Low — but the
audit-friendly `peer_sid` field is stored on `WindowsStream` for every
accepted connection. Fine as-is; noted for completeness.

---

## Positive confirmations

- `SecretString` / `SecretBytes` correctly use `#[derive(ZeroizeOnDrop)]`,
  deny `Clone` in favor of `clone_secret`, implement `ct_eq` equality,
  forbid serde, and redact `Debug`
  (`crates/pcloud-secret/src/secret_string.rs:35-128`,
  `secret_bytes.rs:22-105`). A compile-fail test enforces no serde.
- File vault enforces 0700 parent + 0600 file, atomic tmp rename with
  `O_CREAT|O_EXCL|mode(0600)`, UID check, symlink-resistance test
  (`crates/pcloud-daemon/src/vault/file.rs:138-186,199-221,319-369`).
- Linux IPC uses `SO_PEERCRED` (`crates/pcloud-ipc/src/platform/linux.rs:43-56`),
  BSD/macOS `getpeereid(3)` (unix.rs), Windows named pipe with per-user SID
  DACL + `GetNamedPipeClientProcessId` SID match
  (`crates/pcloud-ipc/src/platform/windows.rs:141-220,244`).
- Production TLS enforcement: `pcloud-config/src/api.rs:137` refuses
  `ApiMode::Plaintext` under `Environment::Production`; IdP exchanger
  rejects non-HTTPS URLs outside loopback
  (`pcloud-idp/src/exchange.rs:158,185`).
- `MAX_REQUEST_BYTES = 1 MiB` cap applied before allocation
  (`crates/pcloud-ipc/src/server.rs:42`).

---

## Recommended follow-ups (priority order)

1. H1 — widen `SecretString` adoption into the runtime method signatures.
2. H2 — wrap `web_token` and drop derived `Debug` on `WebConfig`.
3. M1 — promote `id_token` / `verify_token` / internal `auth_token: &str`
   to `SecretString` at every layer.
4. M3 — zeroize-on-drop for `RedactedString`.
5. M4 — add landlock + seccomp to the Linux daemon.

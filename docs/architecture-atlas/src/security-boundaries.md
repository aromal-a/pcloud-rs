# Security and trust boundaries

## Trust-boundary diagram

```text
untrusted argv / local files / browser / remote replies
             │
             ▼ validation and bounded decoding
┌──────────────── client boundary ────────────────┐
│ CLI / SDK / web: no remote credential authority │
└──────────────────────┬──────────────────────────┘
                       │ owner-authenticated IPC
                       ▼
┌──────────────── daemon trust boundary ────────────────────────┐
│ peer identity │ auth state │ vault │ policy │ audit │ dispatch │
└──────┬───────────────┬───────────────────────┬────────────────┘
       │               │                       │
       ▼               ▼                       ▼
 local durable     crypto key memory      validated TLS endpoint
 state (0600)      zeroize/redaction              │
                                                   ▼
                                             pCloud service
```

## Local IPC

- Unix-family transports use owner-only runtime directories/sockets plus
  kernel peer-credential checks.
- Windows uses an owner-specific named pipe, restrictive DACL, and exact
  client TokenUser SID validation.
- There is no general TCP fallback for daemon control.
- Framing is bounded before body allocation.

File permissions are defense in depth; peer identity is checked even after a
connection reaches the endpoint.

## Secrets and authentication

`SecretString` and `SecretBytes` own zeroizing buffers and redact `Debug`.
Passwords remain ephemeral and are not written to the vault. Durable token
storage is policy-controlled and uses platform facilities where configured:
Secret Service or owner-only file, Keychain, or user-scope DPAPI.

Secret exposure sites should be explicit and short-lived. Do not clone a
secret into a long-lived model merely to satisfy an API shape.

## Remote transport

The protocol crate owns endpoint validation and TLS transport. Production
configuration must reject plaintext downgrade rather than accepting a
“testing” bypass at runtime. API result codes and server-provided messages are
treated as untrusted input and translated into typed, redacted errors.

## Mounted filesystem boundary

Mount options are policy inputs. Dangerous privilege expansion such as an
unrestricted writable `allow_other` mount is rejected. Native adapter code is
a high-risk boundary because kernel callbacks, FFI, path translation, and
writeback durability meet there.

## Plugin and enterprise boundaries

Plugin, Wasmtime, policy, identity-provider, fleet, and KMS crates are
separate bounded subsystems. Their presence does not mean all are enabled or
release-qualified. Evaluate features, manifests, and deployment docs before
moving one into a trusted production path.

## Security reference map

| Concern | Start with |
|---|---|
| Product threat model | `SECURITY-MODEL.md`, `docs/book/src/security/` |
| Disclosure policy | `SECURITY.md` |
| Secret primitives | `crates/pcloud-secret` |
| Vaults | `crates/pcloud-daemon/src/vault/` |
| IPC identity and framing | `crates/pcloud-ipc` |
| TLS/protocol | `crates/pcloud-proto` |
| Content crypto | `crates/pcloud-crypto` |
| Audit/redaction/metrics | `crates/pcloud-observability` |
| Supply chain | `deny.toml`, `audit.toml`, security workflows |
| Historical audit evidence | `.audits`, `AUDIT_REPORT.md`, review archives |

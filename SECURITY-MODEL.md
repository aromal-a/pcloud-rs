# SECURITY MODEL

This document captures the security posture of the Rust rewrite as it is
implemented today. The Rust path is intentionally **stricter** than the
legacy C client in several places; those divergences are called out
explicitly.

Do not read this file as a claim of full parity or production readiness —
see `STATUS.md` for the current parity tally and tracker `bd-1du.10`.

## Trust boundaries

```
  [ User / OS login ]                trusted
        │
        ▼
  [ CLI process ] ─── IPC (uds,0600,peer-uid check) ───► [ Daemon process ]
        ▲                                                     │
        │ SDK (in-proc, same process, no boundary)            │ HTTPS (TLS)
        ▼                                                     ▼
  [ App using pcloud-sdk ]                              [ pCloud API ]
                                                              │
                                                              ▼
                                                       [ Object store ]

  Local state:
    - SQLite store (owner-only file under 0700 dir)
    - Auth vault   (0600 file, 0700 dir)
    - Runtime dir  (0700, IPC socket 0600)
```

Untrusted input surfaces:

- Network: every pCloud response is parsed by typed decoders in
  `pcloud-proto`; malformed payloads return typed errors, never panic.
- IPC: every `Request` is schema-checked; malformed frames terminate the
  offending connection only.
- Local filesystem: path inputs are canonicalized; nested / mount-backed /
  ignored paths are rejected (`mount_discovery.rs`).

## Secrets

Source of truth: `pcloud-secret/src/secret_string.rs`,
`pcloud-secret/src/secret_bytes.rs`.

Rules enforced:

1. **Zeroize on drop** — both `SecretString` and `SecretBytes` clear their
   backing memory on `Drop`.
2. **Redacted `Debug`** — secret wrappers never leak values through
   `format!("{:?}", …)` or tracing spans.
3. **No raw secret storage on long-lived structs.** Auth tokens,
   passwords, crypto master keys, file keys, and temppass material are
   held exclusively through the secret wrappers.
4. **No log of secrets.** `pcloud-observability::logging` redacts fields
   marked as sensitive; CLI input uses non-echoing prompts where possible.
5. **No plaintext password persistence.** Legacy C persisted usernames
   with password-adjacent data; the Rust path rejects that (`bd-1du` note
   under auth).

## Auth token persistence

File: `pcloud-daemon/src/auth_vault.rs`.

- Off by default. Persistence is **explicit opt-in** per profile.
- Vault file mode `0600`; parent dir `0700`.
- Ownership and mode validated on load; mismatched vaults are rejected.
- On rotate / logout, vault entries are replaced or removed atomically
  (temp file + fsync + rename).
- Password is never stored in the vault, even when token persistence is
  enabled.

## IPC permissions

File: `pcloud-ipc/src/{server,transport,auth}.rs`.

- Unix domain socket under `$XDG_RUNTIME_DIR/pcloud-rs/` (fallback:
  user-owned runtime dir).
- Socket file mode: `0600`.
- Parent dir: `0700`.
- Peer UID is read via `SO_PEERCRED` and compared to daemon UID; mismatch
  → connection rejected.
- Slow / malformed clients do not block the server (per-connection
  timeout; framing errors close just that connection).
- Audit persistence failures are **surfaced**, not silently swallowed.

## Filesystem permissions

- SQLite store dir: `0700`, store file: `0600`.
- Staging and page cache dirs: `0700`.
- Journal files: `0600`.
- Runtime dir for sockets / pid files: `0700`.

## Production vs development modes

Controlled by `pcloud-config::ConfigProfile` and `Environment`.

| Concern | Development | Production |
|---------|-------------|------------|
| TLS to pCloud | Required | Required (downgrade **rejected**) |
| Endpoint override | Allowed with validation | Rejected unless signed via profile |
| Token persistence | Opt-in | Opt-in |
| Audit log | stderr permitted | Structured log sink required |
| Crypto `persist_master_key` | Rejected | Rejected |
| `register` local validation | Same | Same |

Production profile explicitly rejects:

- plaintext transport,
- persisting the crypto master key,
- silent audit/persistence failure swallowing,
- raw secret-bearing `String` / `Vec<u8>` in new code.

## Crypto posture

See `pcloud-crypto` crate docs and `ARCHITECTURE.md` — active crypto path
is not fully enabled yet (`bd-1du.5`), but the implemented primitives are:

- Argon2 KEK for password-derived material.
- AES-256-GCM with 12-byte nonce, 16-byte tag.
- Sector index bound into AAD (prevents sector reordering).
- Per-file keys derived via HMAC-SHA256 from master + random seed; master
  key never directly seals file content.
- Fingerprint check on unlock is constant-time (`subtle::ConstantTimeEq`).
- Master key held as `SecretBytes`; wiped on lock.
- `persist_master_key = true` is rejected — plaintext keys never land on
  disk.

## Threat model (brief)

In scope:

- Local attacker with the same UID: blocked where practical (no secret
  persistence by default, vault 0600, socket 0600). Same-UID attacker
  with ptrace capability is **out of scope** — this matches industry
  norm.
- Local attacker with a different UID: blocked at IPC (peer UID check)
  and at FS permissions.
- Network attacker: TLS mandatory. Response parsing is typed; no dynamic
  eval, no shell-outs.
- Malformed pCloud response: typed errors, no panic, no state corruption.
- Malformed IPC request: single-connection isolation.
- Crypto downgrade: production config refuses downgrade away from TLS.

Out of scope:

- Kernel compromise.
- Same-UID attacker with ptrace / process memory read.
- Hardware key extraction.
- Side-channel analysis beyond constant-time fingerprint compare.

## Divergences from C (intentional, security-motivated)

| Area | C behavior | Rust behavior |
|------|------------|---------------|
| Password persistence | Persisted alongside username | Never persisted |
| Auth token persistence | Always on | Opt-in, 0600 vault |
| IPC socket | World-accessible in some builds | 0600, peer-UID checked |
| Audit failure | Often silently ignored | Surfaced as typed error |
| `get_*_value` cross-kind | Returns zero sentinel | `SettingTypeMismatch` |
| Notification callback | Registered C callback | Typed event stream |
| Crypto master key persist | Present as option | Rejected |

These divergences are catalogued under `Rejected` rows in the parity
matrix.

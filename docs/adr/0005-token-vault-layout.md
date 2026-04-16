# ADR 0005: Auth Token Vault Layout and Opt-In Durability

- Status: Accepted
- Date: 2026-04-15

## Context

The C client persists username, password, and auth token in plain
configuration state by default. Reviewer feedback (REVIEW_FULL_02.md,
Security Model) flagged this as an enterprise-unacceptable default: a
compromised backup or a misconfigured home-directory permission leaks
long-lived credentials.

The Rust rewrite must accept a token from interactive or non-interactive
auth, keep it usable for the current session, and — only when the
operator explicitly asks — persist it to disk in a form that is at least
as safe as SSH keys.

Concrete constraints:

- The default must not leak credentials to any other local user.
- The durable path must be opt-in and auditable.
- Passwords are never persisted (see ADR 0007).
- Recovery from a corrupted or ownership-mismatched vault must be safe
  and observable, not silent.

## Decision

The auth vault lives at:

```
$XDG_DATA_HOME/pcloud-rs/auth-vault.json     (default)
~/.local/share/pcloud-rs/auth-vault.json     (XDG fallback)
```

with:

- **parent directory mode `0700`**, owner-only,
- **vault file mode `0600`**, owner-only,
- **owner UID** must match the running process's effective UID; on
  mismatch the vault is refused (not rewritten, not deleted), and the
  daemon logs an audit event and proceeds without a persisted token.

Durability is **opt-in only**, controlled by the environment variable

```
PCLOUD_DURABLE_AUTH_TOKENS=1
```

When unset (the default), the vault is never written to disk; tokens
live in memory inside `SecretString` wrappers (zeroized on drop) and die
with the process. When set, the runtime writes the vault atomically
(`tempfile` → `rename` on the same filesystem, `fsync` on both the file
and the parent directory) so a crash during write cannot produce a
truncated vault.

The vault file carries a small JSON envelope:

- `schema_version: u16`
- `created_at: i64` (unix seconds)
- `token_ciphertext: base64` (reserved for future key-wrapped storage)
- `token_plaintext: base64` (current form; only written when durability
  is opted in; file mode and ownership do the work)

## Consequences

Good:

- Default posture leaks nothing to disk.
- Opting in is a single environment variable; operators running the
  daemon under systemd can enable it via `Environment=` cleanly.
- Ownership / mode mismatch fails closed and loudly, not silently.
- Atomic writes make torn files impossible.

Bad:

- `PCLOUD_DURABLE_AUTH_TOKENS=1` at the current schema writes the token
  in cleartext (inside a `0600` file). That is strictly better than the
  C baseline but weaker than key-wrapped storage. The schema leaves a
  `token_ciphertext` field reserved for a future ADR that introduces an
  OS-keyring-wrapped layout without breaking existing vault files.
- Opt-in means non-interactive first-runs (CI, ephemeral containers)
  must re-authenticate on every run unless the operator deliberately
  enables durability. This is the intended trade-off.

Security invariants enforced in code:

- The file-open path checks `stat` on the parent, then on the file, and
  refuses on unexpected mode or UID before reading any content.
- On write, the tempfile is created with `O_CREAT | O_EXCL` and
  `mode 0600` from the start; the chmod is not a follow-up call that
  could race.
- The in-memory representation is `SecretString` everywhere outside
  the vault file I/O buffer.

## Alternatives Considered

- **Always persist (C behaviour)**: rejected — insecure by default.
- **Never persist**: rejected — operators with legitimate
  non-interactive needs lose usability; explicit opt-in is the right
  middle ground.
- **OS keyring (libsecret / Keychain)**: desirable long-term; deferred
  because the cross-platform surface is non-trivial and the current
  `0600`-plus-opt-in posture already clears the enterprise bar. A
  future ADR will supersede this one when keyring storage lands.

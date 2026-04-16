# ADR 0016: Secret-Wrapping Discipline (`SecretString` / `SecretBytes` / `zeroize`)

- Status: Accepted
- Date: 2026-04-16

## Context

The rewrite handles several classes of in-memory secret material:

- auth tokens and passwords;
- TFA codes and recovery codes;
- crypto unlock passphrases and per-folder data-encryption keys;
- JWTs, PKCE verifiers, client secrets (OIDC broker);
- ed25519 private keys (fleet agent);
- KMS-unwrapped plaintext DEKs (KMS cache);
- Vault / HashiCorp tokens.

The legacy C client holds these as plain `char*` in long-lived
structs, with no guaranteed zeroisation on free and no redaction when
material accidentally reaches a log line. The rewrite cannot inherit
that posture; multiple subsystems (audit, tracing, error reporting)
will be touching these values and must not accidentally leak them.

`pcloud-secret` ships two wrapper types, `SecretString` (UTF-8) and
`SecretBytes` (arbitrary bytes). Both already:

- zeroize on `Drop` via `zeroize::Zeroizing`;
- implement `Debug` as a redacted placeholder (`SecretString([redacted])`);
- do **not** implement `Display`;
- do **not** implement `Clone` without a conscious `clone_secret()`
  call that keeps the wrapper.

This ADR records the **project-wide rule** that governs their use, not
the wrappers' mechanics (documented in the crate).

## Decision

The following is mandatory across every crate in the workspace:

1. Any secret-bearing field on a type that lives longer than a single
   function call must be stored as `SecretString` or `SecretBytes`.
   Raw `String` / `Vec<u8>` is reserved for values that are not
   secrets.
2. Secrets never appear in log output, error messages, `Display`, or
   `Debug` of a containing type. `Debug` derivations on containing
   types must either skip the field, redact it, or hand-implement
   `Debug` with explicit redaction.
3. Secrets never enter OTel span attributes. The attribute allow-list
   on the tracing module (`command`, `duration_ms`, `error_category`,
   `status_code`, `trace_kind`) is enforced; anything outside it is
   dropped in release and panics in debug.
4. Secrets never enter audit-event details. Any audit content that
   must reference a secret value records a **hash** of it under a
   per-installation HMAC key (see the integrity sweeper privacy
   section and the DLP plugin for worked examples).
5. Secrets never enter error messages returned to users. Errors carry
   a taxonomy code (see `ERROR-TAXONOMY.md`); callers translate the
   code to a human message that contains no secret material.
6. Persistence of secrets follows ADR 0015 (`0600` + atomic write +
   owner-only parent) and the token-specific carve-out in ADR 0007.
7. Clippy lint `clippy::unwrap_used` is denied on crates that touch
   secrets, so an accidental `unwrap()` on a secret-bearing `Result`
   cannot leak via panic payload.

## Consequences

Good:

- Every secret has a consistent lifecycle: wrapped on creation,
  zeroised on drop, never stringly-accessed by accident.
- Reviewers can scan a diff for raw `String`/`Vec<u8>` on secret-bearing
  fields and flag it mechanically.
- The audit chain and tracing surfaces are provably
  secret-free-by-construction, not merely by review discipline.
- The rule extends to plugins: the DLP plugin, backup plugin, and
  autoheal plugin all obey the same discipline.

Bad:

- There is boilerplate: callers who legitimately need the cleartext
  bytes call `.expose_secret()` and are immediately visible in review.
  This is intentional — visibility is the point.
- Some ergonomic APIs (e.g. `format!("{:?}", user_struct)`) can
  include a secret-bearing field. The redacted `Debug` on the wrapper
  prevents leakage, but composite types must be careful. Mitigated by
  a `deny(clippy::missing_debug_implementations)` policy combined with
  manual review on any struct that contains a wrapper.

## Alternatives Considered

- **`secrecy` crate**: considered; our wrappers predate its adoption
  and we chose to keep the minimal in-repo type so we own the `Debug`
  surface and the zeroisation invariant. A future ADR may migrate.
- **Convention only ("don't log secrets, please")**: rejected — this
  is exactly the C client's posture and it leaks on any ad-hoc
  `printf` a future contributor adds.
- **Encrypt-in-memory (e.g. `memguard`-style)**: considered for the
  crypto DEK cache; deferred. The wrapper + zeroise-on-drop rule is
  the universal baseline; specific subsystems may add stronger
  protection on top.

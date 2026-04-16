# ADR 0015: `0600` Vault File Permission Enforcement

- Status: Accepted
- Date: 2026-04-16

## Context

The daemon persists at least two kinds of sensitive material to disk:

- the auth vault (`pcloud-daemon::auth_vault`) — wraps the current
  session token when durable persistence is opted into (ADR 0005);
- the fleet device-identity key (`pcloud-fleet::MtlsFleetAgent`) — an
  ed25519 private key used to sign fleet control-plane requests.

Both must be unreadable by other local users. The C client historically
relied on the user's umask to produce reasonable perms, which is a soft
guarantee at best: an operator with a liberal umask, a misconfigured
systemd unit, or a careless migration tool can silently leave the file
world-readable.

ADR 0005 already specifies the layout (`0600` file under a `0700`
parent). This ADR records the broader rule, the **enforcement** rather
than the layout, and extends it to every secret-bearing file the
rewrite writes to disk.

## Decision

Every secret-bearing file written by `pcloudd`, `pcloud-sdk`, or a
first-party plugin must satisfy all of the following, enforced at
**both write-time and load-time**:

1. File mode is exactly `0o600`. Wider modes (`0640`, `0644`, …) are
   refused on load with a structured error; they are never silently
   re-tightened, because silent re-tightening hides a security
   regression in the caller.
2. Parent directory mode is `0o700`.
3. File owner matches the current effective UID. Foreign-owned secret
   files are refused.
4. On Windows, an equivalent ACL check is enforced via the
   `pcloud-secret` platform layer (Keychain / DPAPI / ACL-restricted
   file fallback); the `0o600` check becomes a
   `current-user-only-DACL` check.
5. Writes go through an **atomic** path: `write(file.tmp)` →
   `fsync(file.tmp)` → `rename(file.tmp, file)` → `fsync(parent-dir)`.
   `file.tmp` is created with `O_CREAT | O_EXCL` and mode `0o600`
   from the first syscall.

The set of files currently covered:

- `auth_vault` persistence file (ADR 0005);
- `pcloud-fleet` device-identity key;
- `pcloud-plugin-publink-expiry` rate-limit state JSON;
- every on-disk snapshot produced by the backup subsystem.

## Consequences

Good:

- Silent permission regressions become loud: a migration tool or a
  careless `chmod` surfaces as a daemon load error, not a one-line
  disclosure in a pen-test report.
- Atomic writes prevent "torn" secret files (partial file on crash
  exposing the old content alongside the new) and eliminate the
  "backup file with wide perms" class of issue.
- The rule is mechanical — easy to audit, easy to enforce in CI, easy
  to port to Windows via the platform abstraction.

Bad:

- Loading a legitimately wide vault (e.g. one imported from the C
  client) fails rather than auto-tightens. Mitigated by
  `pcloudc migrate-from-c`, which re-writes the file under the strict
  rule rather than silently modifying it in place.
- On Windows the equivalence is "close to but not identical to"
  POSIX `0600`; the documentation is explicit about the mapping.
- Write amplification: two `fsync` calls per secret update. Secret
  files are small and infrequently updated; the cost is negligible
  at observed rates.

## Alternatives Considered

- **Trust the user's umask**: rejected — this is exactly what the C
  client did, and it produces variable, invisible, non-audit-friendly
  outcomes.
- **Silently `chmod` on load**: rejected — erases the evidence of a
  security regression and hides the bug from whoever wrote the file
  wide in the first place.
- **Defer to OS keyring only**: considered — Keychain / DPAPI / Secret
  Service are preferred on desktops (see the cross-platform wave),
  but the file fallback is load-bearing on headless Linux and BSD
  boxes, so the rule still applies there.

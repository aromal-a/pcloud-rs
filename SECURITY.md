# Security Policy

<!-- Purpose: responsible-disclosure policy and scope statement for the pcloud-rs Rust rewrite. -->

The pcloud-rs Rust rewrite (`` workspace) takes security seriously.
This document tells you how to report vulnerabilities privately, what is in
scope, what is explicitly out of scope, and what known issues we currently
carry. The authoritative audit record is
[`SECURITY-AUDIT-FINAL-14042026.md`](./SECURITY-AUDIT-FINAL-14042026.md).

For the structured security model (trust boundaries, threat model, secret
handling, mount policy, transport policy, and `unsafe` justification
rules), see the mdBook security chapter at
[`docs/book/src/security/`](./docs/book/src/security/) and the top-level
[`SECURITY-MODEL.md`](./SECURITY-MODEL.md).

## Reporting a Vulnerability — Private Channel

**Do NOT open a public GitHub issue, pull request, discussion, or commit for
anything that looks like a vulnerability.**

Instead, use one of the following private channels:

1. **GitHub Security Advisories (preferred)** — open a draft advisory on
   the repository under **Security → Advisories → New draft advisory**.
   This keeps the report embargoed and visible only to maintainers.
2. **Encrypted email** — send to the maintainer listed in the repository
   `Cargo.toml` / `README.md`. PGP-encrypted mail is preferred. If you need
   a key and cannot find one, say so in the first (plaintext) message and
   we will respond with a key before you send any sensitive details.

When you report, please include:

- a clear description of the issue and the affected crate/file (e.g.
  `crates/pcloud-fs/src/http_download.rs`),
- reproducer or proof-of-concept (redacted of personal data),
- your assessment of impact and exploitability,
- the version / commit hash you tested against,
- whether you want to be credited and under which name.

### Response targets

- Acknowledgement: within **3 business days**.
- Initial triage + severity assessment: within **7 business days**.
- Fix or documented mitigation for High/Critical: best-effort within
  **30 days** of triage; longer if upstream dependencies block the fix
  (see H6-1 below).
- Coordinated disclosure window: typically **90 days** from triage, earlier
  if a fix ships sooner, extended by mutual agreement.

We will credit reporters in the release notes / `CHANGELOG.md` unless you
ask to remain anonymous.

## In Scope

The following areas of the `` workspace are in scope for
security reports:

- Auth flows: password, token, TFA code, recovery code, TFA SMS/device
  resend (`crates/pcloud-auth`, `crates/pcloud-daemon/src/auth_backend.rs`,
  `crates/pcloud-daemon/src/auth_vault.rs`).
- Local IPC transport: socket permissions, peer credential checks,
  malformed-frame handling (`crates/pcloud-ipc`).
- Config / endpoint validation: production TLS enforcement, endpoint
  override validation (`crates/pcloud-config`).
- Secret handling: wrappers, zeroization, redaction, non-persistence
  (`crates/pcloud-secret`, auth vault, CLI input).
- Crypto operations: AES-256-GCM sector sealing, Argon2 KDF,
  HMAC-SHA256/512, constant-time comparison usage
  (`crates/pcloud-crypto`).
- Filesystem / mount surface: mount policy, path normalization, read /
  write paths, staging, journal, writeback
  (`crates/pcloud-fs`).
- Protocol client parsing: response frames, fuzz-reachable decoders
  (`crates/pcloud-proto`).
- SDK and CLI exposed APIs (`crates/pcloud-sdk`, `crates/pcloud-cli`).

## Out of Scope / Will Not Be Accepted

We will close the following classes of report as **Not a vulnerability**
or **Rejected by design**:

- Requests to enable `allow_other` on a writable FUSE mount. The Rust
  path explicitly rejects `allow_other && !read_only` in
  `MountService::validate`; this is a hard security invariant, not a bug.
  Equivalent requests for `allow_root` or `setuid` mounts will also be
  rejected.
- Requests to reintroduce cleartext password persistence in the auth
  vault. The Rust rewrite intentionally does not mirror that legacy C
  behavior (see `CLAUDE.md` → *Auth token persistence* rules).
- Requests to weaken IPC socket permissions below `0600` / parent `0700`,
  or to drop the `SO_PEERCRED` UID check.
- Requests to enable plaintext transport under
  `Environment::Production`. The central
  `ApiEndpoint::validate(environment)` gate rejects this by design.
- Requests to add `danger_accept_invalid_certs`,
  `accept_invalid_hostnames`, or any custom certificate-validator
  shortcut.
- Findings that require already-root local access to exploit and do not
  cross a trust boundary (unless they escalate into a remote impact).
- Missing rate-limit or abuse protection for self-hosted/self-operated
  endpoints where the threat model is the user themselves.
- Theoretical cryptographic downgrade reports against primitives the
  code does not actually negotiate.
- Update-check, auto-update, or telemetry surfaces. Those are ghost
  declarations from the upstream C fork and are intentionally
  **Rejected** in the Rust rewrite. See
  [`REJECTED-RATIONALES-14042026.md`](./REJECTED-RATIONALES-14042026.md).
- Parity-gap reports against features documented as `Missing` or
  `Partial` in `C_FEATURE_PARITY_MATRIX.csv`. Those are tracked feature
  gaps, not vulnerabilities.

If you believe a rejected class above is actually exploitable in a way we
have not considered, please still report it privately with a concrete
exploit path — we will reconsider on evidence.

## Known Open Security Issues

### H6-1 (High, Carried) — `fuser 0.15.1` RUSTSEC-2021-0154 (unsound)

- **Advisory**: [RUSTSEC-2021-0154](https://rustsec.org/advisories/RUSTSEC-2021-0154.html)
- **Affected file**: `crates/pcloud-fs/Cargo.toml` (workspace dep;
  reach sites in `crates/pcloud-fs/src/mount_service.rs` and the
  `fuser_shim.rs` wrapper).
- **Status**: No patched upstream `fuser` release exists.
- **Mitigation**:
  - time-boxed ignore scoped to `bd-1du.4` in `audit.toml`,
  - `FuserShim` surface kept minimal until full mount parity lands,
  - production mounts gated behind an explicit opt-in until a fixed
    `fuser` release ships,
  - exploitability requires a real mount (scaffolding only today).
- **Tracking**: `bd-1du.4` in the `bd` tracker; details in
  [`SECURITY-AUDIT-FINAL-14042026.md`](./SECURITY-AUDIT-FINAL-14042026.md)
  §Carried-Open Findings.

### L1 (Low, New wave 9) — `dwltag` cookie value not CRLF/whitespace-validated

- **File**: `crates/pcloud-fs/src/http_download.rs` (`build_request`).
- **Risk**: theoretical HTTP request-splitting if the pCloud edge ever
  returned a `dwltag` containing `\r\n`. Current source is trusted signed
  link metadata; exploitability today is effectively zero.
- **Planned fix**: reject any byte outside the token-safe range
  (`0x21..=0x7E`) for `dwltag`, `host`, `path` before embedding. See
  [`SECURITY-AUDIT-FINAL-14042026.md`](./SECURITY-AUDIT-FINAL-14042026.md)
  §New Findings.

## Security Posture Summary

A full security audit (secrets handling, IPC, transport, mount policy,
crypto primitives, file permissions, `unsafe` justification, logging
redaction, TLS-bypass absence) was completed at wave 9 and recorded in
[`SECURITY-AUDIT-FINAL-14042026.md`](./SECURITY-AUDIT-FINAL-14042026.md).
The overall posture was assessed **top-of-the-line for the retained &
implemented surface**, with the two findings listed above as the only
open items.

## Honesty Discipline

Per [`CLAUDE.md`](../CLAUDE.md), the project refuses to claim

- "full parity",
- "production ready",
- "enterprise ready",
- "drop-in replacement"

until `bd-1du.10` is satisfied by code, tests, docs, and parity-matrix
evidence. Any security claim in documentation must be backed by tests
or by the audit file referenced above.

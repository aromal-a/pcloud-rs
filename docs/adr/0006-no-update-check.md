# ADR 0006: No Built-In Update Check

- Status: Accepted
- Date: 2026-04-15

## Context

The C codebase declares `psync_check_new_version`, `psync_check_new_version_download`,
and related helpers in `psynclib.h`. Reviewer 19 (REVIEW_FULL_02.md) asked
whether the Rust rewrite should mirror these.

On examination of the C tree:

- The update-check declarations exist but the server endpoint they
  target is not present in the fork's recent history, and the
  implementation is largely stubbed or behind feature gates that are
  never enabled in the shipped client.
- Even where the implementation is live, the behaviour is a plain HTTP
  version-check with no signature verification, no pinning, and no
  explicit user consent; an attacker who can redirect that request can
  trigger a download prompt.
- This is a community fork. Operators install via distro packages,
  OS-level package managers, `cargo install`, or `pacman`/`apt` — all of
  which have their own signed-update channels.

## Decision

The Rust rewrite **does not** implement update-check or self-update
functionality. The corresponding rows in
`C_FEATURE_PARITY_MATRIX.csv` are marked `Rejected`, with the rationale
attached in `REJECTED-RATIONALES-14042026.md`.

Concretely, the following C surfaces are intentionally absent from the
Rust tree and will not be added:

- `psync_check_new_version`
- `psync_check_new_version_download`
- any auto-apply / self-replace variant of the above

## Consequences

Good:

- Zero attack surface for a "phone-home + fetch executable" path in the
  daemon. No DNS lookups, no HTTP clients, no sha256 chains to mismanage.
- Zero ambiguity for packagers: the daemon never competes with
  `dpkg` / `rpm` / `pacman` / `cargo install --force`.
- Reduced enterprise friction: air-gapped deployments do not need a
  firewall rule or a config toggle to suppress background update
  traffic.
- Simpler threat model for the runtime: no code path that can be
  coerced into running new code inside the daemon without an operator
  restart.

Bad:

- Users who rely on the C client's prompt to know a new version exists
  lose that prompt. Mitigation:
  - `pcloud-rs --version` prints the current version.
  - Release notes live under `CHANGELOG.md`.
  - Distro-level upgrade hooks (apt, pacman, etc.) surface updates in
    the normal way.

Policy:

- PRs that add version-poll, update-check, or self-update code are
  rejected on principle. A follow-on ADR that supersedes this one
  would be required to change the decision, and would need to address
  signature verification, trust root, and consent UI before being
  accepted.

## Alternatives Considered

- **Mirror C behaviour (plain HTTP version check)**: rejected — adds an
  unsigned phone-home with no operational upside.
- **Signed update channel (sigstore / minisign)**: rejected for this
  fork — the infrastructure cost (release signing, rotation, revocation,
  trust-root distribution) is disproportionate for a community rewrite
  whose users install via distro channels anyway.
- **Opt-in update check**: rejected — even opt-in, the daemon would
  ship the networking and file-replace code, growing the attack
  surface permanently. Distro packaging covers the use case without
  that cost.

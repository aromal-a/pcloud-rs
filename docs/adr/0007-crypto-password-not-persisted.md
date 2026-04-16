# ADR 0007: Crypto Password Is Never Persisted

- Status: Accepted
- Date: 2026-04-15

## Context

pCloud Crypto ("Crypto Folder") is an end-to-end encrypted zone backed by
an RSA keypair whose private key is itself encrypted with a
password-derived key. The C client optionally caches the crypto password
alongside the account password in its local configuration so the crypto
folder can be unlocked automatically after daemon restart.

That convenience directly undermines the security model of Crypto:

- An attacker who lifts the config file from a backup gets both the
  account credentials and the crypto unlock material.
- Full-disk encryption of the home partition does not help — anyone
  with read access to the file in a running session reads the
  plaintext.
- The server's threat model assumes the crypto password never leaves
  the user; persisting it locally breaks that assumption unilaterally.

Reviewer feedback (SECURITY-MODEL.md, Section "Retained Security
Carve-outs") was unambiguous: the C behaviour is convenient but
incorrect from an enterprise-security standpoint.

## Decision

The Rust rewrite **never** writes the crypto password to disk. There is
no configuration flag, no environment variable, and no debug switch that
enables persistence. The corresponding C surface
(`psync_crypto_save_password` / `psync_crypto_load_password` and any
analogue) is marked `Rejected` in the parity matrix with this ADR cited
as the rationale.

The live behaviour is:

- The crypto password is accepted interactively (CLI prompt) or via a
  one-shot IPC request from a trusted local peer.
- It is held in a `SecretString` for the duration of the unlocked
  session and zeroized on drop, on lock, and on daemon shutdown.
- A daemon restart requires the user to re-enter the crypto password.
- The account auth token may still be persisted under the opt-in
  durability rules of ADR 0005; these are two independent knobs, and
  there is no shared persistence path.

## Consequences

Good:

- Crypto's end-to-end property is preserved across the local client
  boundary. A stolen home directory does not yield cleartext crypto
  material.
- The crypto code path has exactly one source of the password
  (interactive/IPC input) and one destination (the in-memory key
  derivation). Audit is trivial.
- Backups of `~/.local/share/pcloud-rs/` are safe to ship to third-party
  backup systems without additional masking.

Bad:

- Users restarting the daemon must re-unlock their crypto folder. For
  interactive users this is acceptable; for service-like deployments
  it is a real usability cost.
- Parity tooling cannot be silenced by implementing the C surface; the
  parity matrix explicitly documents this row as `Rejected` and the
  CI parity checker treats `Rejected` as terminal, not "TODO".

Operational guidance:

- Operators who need automatic unlock must use OS-level facilities
  (systemd `systemd-ask-password`, a keyring, or a TPM-sealed blob
  outside this daemon's scope). A future ADR may integrate with such a
  facility; until then the daemon refuses to grow this surface.

## Alternatives Considered

- **Mirror C behaviour, persist crypto password**: rejected —
  fundamentally incompatible with the Crypto threat model.
- **Persist encrypted with a user-supplied key**: rejected — shifts the
  problem by one hop (the wrapping key must then persist) without
  buying anything meaningful. An OS keyring (ADR candidate) is the
  correct long-term answer.
- **In-memory cache across "soft" restarts**: rejected — `systemd`
  restart, crash, reboot, and explicit `pcloudctl stop` are all
  operationally equivalent from the security standpoint; carving out
  "some" restarts as cached invites exactly the bugs the whole
  decision is trying to avoid.

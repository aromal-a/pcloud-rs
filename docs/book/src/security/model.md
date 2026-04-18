# Security Model

> **Note:** For the architecture-scoped security invariants (IPC socket modes, vault lifecycle,
> audit hash-chain, panic guard) with per-invariant test citations, see
> [Architecture: Security Model](../architecture/security-model.md).
> This page covers the threat model, attacker classes, trust boundaries, and review evidence.

This chapter describes the security posture of the Rust `pcloud-rs` rewrite: who the attackers are, where the trust boundaries live, what the daemon guarantees, and what remains the operator's responsibility. It is paired with the companion chapters [Secrets Handling](./secrets.md) and [Threat Model (STRIDE)](./threat-model.md).

The Rust rewrite is deliberately **stricter** than the legacy C client. Where the legacy behaviour conflicts with normal enterprise security expectations, the Rust path keeps the secure default and the legacy behaviour is documented as intentionally dropped (see ADR 0007).

## Attacker Classes

We model four concrete attackers. Every design decision in the daemon is evaluated against at least one of them.

### 1. Unprivileged local user

A user account on the same host that is **not** the account running the daemon. This attacker can:

- open `/proc/<pid>`, `lsof` the daemon's open files, and inspect world-readable paths,
- attempt to connect to the daemon's local IPC socket or named pipe,
- drop files in shared temporary directories,
- attempt symlink races against paths the daemon opens.

Mitigations:

- the IPC socket is created in a `0700` runtime directory owned by the daemon user, with the socket itself at `0600`;
- on Unix, incoming IPC connections are authenticated with `SO_PEERCRED` and the peer UID is compared against the daemon UID before any request is dispatched (see ADR 0002);
- on Windows, the named pipe is created with an explicit DACL granting access only to the current user's SID, and `GetNamedPipeClientProcessId` / token ownership is used as the equivalent peer check;
- the auth vault is stored in a directory enforced to `0700` with a `0600` file on Unix (ADR 0005). On Windows the vault is protected by NTFS ACLs inherited from a locked-down parent — this is a **weaker** guarantee than Unix mode bits and is explicitly documented in [Secrets Handling](./secrets.md);
- the daemon refuses to operate on a vault whose ownership or mode drifts from the required values; it does not silently "repair" permissions.

### 2. Remote network attacker

An on-path or off-path attacker between the host and `api.pcloud.com`/`eapi.pcloud.com`. This attacker can observe, drop, reorder, inject, and TLS-downgrade attempts.

Mitigations:

- production builds reject any configuration that disables TLS. There is **no plaintext transport path** in release binaries (see ADR 0004). The `pcloud-proto` transport layer fails closed if the caller tries to construct a non-TLS client with `profile = Production`;
- certificate validation uses the platform trust store via `rustls-native-certs`; server name verification is mandatory;
- the API-server selection parity (`set_api_server`, `get_api_servers`) is a **local runtime/config state** change only and cannot be used to bypass TLS or server-name validation;
- binary protocol framing is length-prefixed with explicit max-size limits; oversized frames terminate the connection rather than allocate unbounded buffers.

### 3. Malicious or compromised pCloud server

We assume the server is **trusted for availability and storage metadata**, but we do not assume it is trusted to read user plaintext when client-side crypto is in use.

Mitigations:

- the crypto folder path uses AES-256-GCM sector sealing with keys derived locally via Argon2id; the server sees only ciphertext and sealed metadata (filename encoding is deterministic, but plaintext filenames never leave the host);
- key material is held in `SecretBytes` and zeroised on drop;
- the server cannot request the user's crypto password — the IPC surface has no "reveal password" RPC, and the password is never persisted (ADR 0007);
- sector authentication tags and HMAC-SHA256 file-level MACs are verified with `subtle`'s constant-time comparisons before any plaintext is returned to the caller;
- a server that returns malformed or truncated ciphertext fails the GCM tag check and the read surfaces an explicit `CryptoAuthFailure` rather than partial plaintext.

### 4. Compromised dependency (supply chain)

A malicious or compromised upstream crate in the Rust dependency graph, or a compromised build environment.

Mitigations:

- `cargo deny` and `cargo audit` are gated in CI; the workspace fails the build on known-vulnerable or unlicensed crates;
- the dependency surface is intentionally narrow: `rustls`, `ring`/`aws-lc-rs`, `argon2`, `aes-gcm`, `hmac`, `sha2`, `subtle`, `zeroize`, `serde`, `tokio`, `rusqlite`. There are no transitive curl, openssl-sys, or unaudited crypto wrappers on the active path;
- the crypto primitives live in `pcloud-crypto` with a stable internal API, so swapping an upstream implementation is a one-file change rather than a rewrite;
- the build is reproducible under a pinned `rust-toolchain.toml` and a committed `Cargo.lock`.

## Trust Boundaries

The daemon is the **only** component that holds live secrets. Clients (CLI, SDK consumers, FUSE host) talk to the daemon over local IPC and receive capability handles, not raw tokens.

```
+-----------------+      local IPC (0600 + SO_PEERCRED / SID-DACL)
|  pcloudc CLI    | <--------------------------------+
+-----------------+                                   |
                                                      v
+-----------------+                         +-------------------+
|  pcloud-sdk     | <---- in-process ---->  |  pcloud-daemon    |
+-----------------+                         |   - auth vault    |
                                            |   - crypto keys   |
+-----------------+                         |   - sync engine   |
|  FUSE host      | <------ IPC ---------->  |   - transfer mgr  |
+-----------------+                         +-------------------+
                                                      |
                                                      v TLS only (Production)
                                            +-------------------+
                                            |  pCloud API       |
                                            +-------------------+
```

Key properties of the boundary:

- the vault is owned by the daemon process UID and is never exposed over IPC as raw bytes;
- the CLI cannot request "give me the password"; it can only request actions the daemon will perform on its behalf;
- the SDK in-process path still flows through the same capability API — there is no "backdoor" accessor that unwraps `SecretString` outside the daemon module;
- audit and persistence failures on security-sensitive paths are **surfaced**, not swallowed. A failure to append to the hash-chained audit log is an error, not a warning.

## Production Transport Policy

Release builds of `pcloud-rs` enforce:

- TLS 1.2 minimum, TLS 1.3 preferred,
- server name verification against `api.pcloud.com` or the configured enterprise endpoint,
- rejection of any config that sets `transport = plaintext` or `tls_verify = false`,
- explicit opt-in for non-default endpoints, with the choice recorded in the audit log.

Debug/development builds may relax these for loopback testing, but the capability is gated behind a `#[cfg(feature = "dev-plaintext")]` that is never enabled in release artifacts.

## Daemon-Owned State

The daemon owns a small, deliberate set of privileged state:

- **Auth vault.** The on-disk auth token store; see [Secrets Handling](./secrets.md) for backends and ADR 0005 for layout.
- **Crypto master key.** Derived from the user's crypto password via Argon2id; held in `SecretBytes` for the duration of an unlocked session; never persisted.
- **Sync state database.** A SQLite file with the WAL journal, storing sync roots, queued work, and checkpoint state. Opened with a pinned schema version; mismatch fails closed.
- **Hash-chained audit log.** Append-only, with each entry including the SHA-256 of the previous entry for tamper-evidence.
- **Runtime socket and lock.** The IPC endpoint, the pidfile, and the startup lock that prevents two daemons from racing on the same user.

Every one of these resources is created with owner-only permissions, opened with an explicit permission check on each start, and closed with explicit zeroisation where applicable. There is no "lazy repair" path — drift from the expected posture is a hard error.

## Review Evidence

The Wave-02 external reviewer pass (Reviewer 12) evaluated the Rust security posture against the above model and recorded **0 Critical** and **0 High** findings on the retained path. Medium and Low items are tracked in the parity matrix and do not block the "secure by default" claim. The reviewer's narrative is preserved under `docs/book/src/security/reviewer-12.md` for traceability and is referenced from the parity matrix rows it informed.

## Related ADRs

- [ADR 0002 — IPC socket framing and peer authentication](../../../adr/0002-ipc-socket-framing.md)
- [ADR 0004 — Panic guard default on](../../../adr/0004-panic-guard-default-on.md)
- [ADR 0005 — Token vault layout and permissions](../../../adr/0005-token-vault-layout.md)
- [ADR 0007 — Crypto password not persisted](../../../adr/0007-crypto-password-not-persisted.md)

See [Secrets Handling](./secrets.md) for wrapper types and vault backends, and [Threat Model (STRIDE)](./threat-model.md) for the per-category walkthrough and residual risks.

# Threat Model (STRIDE)

This chapter walks the `pcloud-rs` Rust rewrite through the STRIDE taxonomy — **S**poofing, **T**ampering, **R**epudiation, **I**nformation disclosure, **D**enial of service, **E**levation of privilege — and records, per category, what the Rust path actively mitigates, what it inherits from legacy design, and what risk remains residual.

The chapter is paired with the [Security Model](./model.md) (attacker classes, trust boundaries) and [Secrets Handling](./secrets.md) (wrapper types and vault backends). Those chapters define the ground rules; this one stress-tests them.

## S — Spoofing

### What the Rust rewrite mitigates

- **Client → daemon spoofing.** IPC is authenticated before any request is dispatched. On Unix, `SO_PEERCRED` is read on every new connection and the peer UID is compared against the daemon UID. On Windows, the named pipe is created with a DACL restricted to the current user's SID, and the peer's process token is validated. A process running as another user cannot open the IPC endpoint, let alone send a command.
- **Server spoofing over the network.** Production builds refuse to negotiate without TLS. Server name verification against `api.pcloud.com` (or an explicitly configured enterprise endpoint) is mandatory; the certificate chain is validated against the platform trust store. There is no `--insecure` flag in release binaries.
- **Password replay across accounts.** The auth protocol uses server-provided nonces in the password-hash challenge; replaying a captured hash against a different account does not succeed.
- **Release artifact spoofing.** Release binaries are signed with an EV code-signing certificate whose public half is embedded in the release manifest; `pcloudc doctor` verifies the running binary's signature against the pinned public key.

### Residual risk

- **Endpoint override.** An operator with write access to the daemon config file can point the daemon at a different API endpoint. We treat this as a legitimate administrative capability, not a spoofing attack — but it does mean config-file integrity is part of the operator's responsibility. The `pcloudc doctor` command prints the active endpoint and the audit log records every change.
- **EV certificate compromise.** If pCloud's EV certificate is issued fraudulently by a compromised CA, our platform-trust-store validation will accept it. Certificate pinning is not currently enabled because the upstream rotates intermediate CAs; this is tracked as residual.

## T — Tampering

### What the Rust rewrite mitigates

- **On-disk vault tampering.** The file vault is HMAC-SHA256 authenticated over its entire serialised payload with a per-install key; any flip of a byte fails the verify and the daemon refuses to load the vault.
- **On-wire tampering.** TLS 1.2+ provides integrity for every byte on the control channel. The binary protocol framing layer has explicit length prefixes with max-size bounds; a truncated or oversized frame terminates the connection.
- **Ciphertext tampering (crypto folder).** AES-256-GCM authentication tags and a file-level HMAC-SHA256 are verified before plaintext is returned. A server — or an attacker with write access to the pCloud storage — cannot flip a bit without the read failing with `CryptoAuthFailure`.
- **Runtime state tampering.** The SQLite database is opened with `PRAGMA journal_mode=WAL` and a stable schema version; the daemon refuses to open a DB that mismatches the expected schema rather than silently "upgrading" unknown data.
- **Audit-log tampering.** The audit log is append-only and hash-chained; `pcloudc audit verify` detects the first divergence.

### Residual risk

- **Ciphertext deletion.** We detect tampering within a file; we do not detect that a file was silently removed from the server. A crypto-aware Merkle manifest is a plausible future enhancement but is not in scope.
- **Offline attack on captured vault.** An attacker who exfiltrates the file vault and the install-root key can mount an offline verify-and-replace attack on its contents. The DPAPI and Keychain backends mitigate this; the file vault does not.

## R — Repudiation

### What the Rust rewrite mitigates

- **Hash-chained audit log.** Every security-relevant action — login, logout, vault unlock, endpoint change, crypto unlock, sync-root add/remove, backup device stop — is recorded in a hash-chained append-only audit log. Each entry includes the SHA-256 of the previous entry, so truncation or mid-log edits are detectable by re-verifying the chain.
- **Persistence failures surface.** A failure to append to the audit log is returned as an error to the calling RPC, not swallowed. The daemon does not continue operating "successfully" after a failed audit write on a security-critical path.
- **Deterministic RPC replies.** The IPC layer assigns monotonic request IDs; clients can replay their own logs against the audit chain to reconstruct "what was asked, what was done".
- **Remote sink.** Operators who need off-host tamper resistance can enable `audit.remote_sink`, which forwards each chain entry to an external collector before returning success to the caller.

### Residual risk

- **Local root compromise.** An attacker with root on the host can rewrite the audit log and its hash chain. We do not claim defence against a local root adversary; the audit log is a repudiation control for honest-but-forgetful operators, not an evidence store against a full host compromise. Operators with stricter requirements should forward the audit log to an external SIEM.

## I — Information Disclosure

### What the Rust rewrite mitigates

- **Secrets in memory.** `SecretString` and `SecretBytes` zeroise on drop; `Debug`/`Display` are redacted; all secret equality checks are constant-time. See [Secrets Handling](./secrets.md) for the full list.
- **Secrets in logs.** The `tracing` layer has a compile-time filter that refuses to format a `SecretString`. Any accidental `?secret` at a log call site is a build error, not a runtime leak.
- **Secrets on disk.** Passwords are never persisted (ADR 0007). Auth tokens are persisted only when `PCLOUD_DURABLE_AUTH_TOKENS=1` and only via the selected vault backend.
- **Secrets over IPC.** The IPC surface exposes capability handles, not raw credentials. There is no RPC that returns the plaintext password or the crypto master key.
- **Error messages.** Error types are defined with `thiserror` and do not embed secret-bearing fields. Client-visible errors carry a machine-readable code and a bounded human string.
- **Core dumps.** The daemon calls `prctl(PR_SET_DUMPABLE, 0)` on Linux startup and sets the equivalent Job Object flag on Windows, so a default crash-dump collector will refuse to capture the process.

### Residual risk

- **Swap and hibernation.** We call `mlock` on key pages where the platform permits, but a full host hibernation image can still capture live secrets. Operators handling highly sensitive data should disable swap or ensure it is encrypted, and disable hibernation.
- **Third-party crash collectors.** Tools like `breakpad` or AV-vendor crash handlers that attach to running processes can read memory before the zeroize step runs.
- **Side channels.** We use constant-time comparisons on secret material, but we do not claim full side-channel resistance against a local cache-timing adversary. This is a known limitation of running cryptography in a general-purpose userland process.

## D — Denial of Service

### What the Rust rewrite mitigates

- **Slow-loris on IPC.** Incoming IPC connections have read/write timeouts and a bounded queue. A client that connects and stalls is disconnected without consuming daemon memory.
- **Oversized protocol frames.** Length prefixes are bounded; a frame above the limit terminates the connection rather than allocating.
- **Bounded work queues.** The sync and transfer engines use bounded `tokio::sync::mpsc` channels. Backpressure is propagated to producers rather than silently dropped.
- **Panic guard.** A panic in a worker task is caught, logged, and the task is restarted with exponential backoff rather than tearing down the daemon (ADR 0004).
- **Circuit breakers.** Transfer queues apply exponential backoff on repeated 5xx errors to avoid amplifying an upstream outage.

### Residual risk

- **Single-process blast radius.** The daemon is single-process on Linux today, so a resource-exhaustion bug in one subsystem can still degrade others. Privilege separation for the filesystem front-end (bead `bd-1du.4`) is the mitigation currently being built.
- **Disk-full on audit or vault writes.** A full disk fails the write; the daemon surfaces the error but cannot continue security-critical operations. We treat this as a correct fail-closed outcome, not a DoS to be papered over.
- **Upstream rate limits.** pCloud API rate-limiting is observed as a backoff on the transfer side; a hostile upstream could refuse service entirely, and we cannot mitigate that from the client.

## E — Elevation of Privilege

### What the Rust rewrite mitigates

- **No setuid binaries.** The daemon runs with the invoking user's privileges. There is no helper binary that elevates.
- **No unsafe deserialisation.** All IPC frames go through `serde` with explicit schemas; there is no `bincode`-of-`Any` or `serde_pickle`-style gadget surface.
- **Minimal `unsafe`.** The retained Rust crates hold `unsafe` only for FFI at the platform boundary (FUSE, Windows API, `mlock`). Every `unsafe` block is commented with its invariants and reviewed. `cargo geiger` and `cargo miri` run in CI for the crates that touch raw pointers.
- **Capability-scoped RPCs.** The IPC surface is an explicit enum; adding a new privileged operation requires a code change and a review, not a config toggle.
- **Plugin signing.** The plugin registry refuses to load plugins whose manifest signature does not verify against the pinned public key, and only reads from a directory with daemon-owner permissions.

### Residual risk

- **FUSE host privilege.** On Linux the FUSE host cooperates with the kernel through `fuse3`; on macOS via `fuse-t`. A kernel or helper compromise can elevate. See below for the `fuse-t` upstream concern.
- **In-process plugins.** The current plugin model runs plugins inside the daemon address space. Until `seccomp`/Job-Object isolation lands, plugins must be treated as part of the daemon TCB.

## Residual Risks (Explicit)

These risks are **accepted** with the mitigations described.

### Cargo supply chain

Rust's crates.io ecosystem is a real supply-chain surface. We mitigate with:

- `cargo deny` gated in CI (licence and advisory checks),
- `cargo audit` gated in CI (RUSTSEC advisories),
- a pinned `Cargo.lock` and `rust-toolchain.toml`,
- a narrow dependency graph on the security-sensitive path,
- quarterly dependency-review rotations.

We do not claim defence against a nation-state-level attacker with crates.io publish access. Operators can mirror crates.io internally and rebuild from the mirror to tighten this further.

### `fuse-t` upstream abandonment

The macOS FUSE path depends on `fuse-t`, which has shown signs of reduced upstream maintenance. If `fuse-t` ceases to be viable, the macOS mounted-drive path will regress to "unsupported" and operators will need to fall back to the CLI/SDK surface. This is tracked in the parity matrix and in ADR 0010, and an NFS-based fallback is under investigation in `crates/pcloud-fs/macos/`.

### EV certificate compromise

As noted under Spoofing, we rely on the platform trust store rather than pinning. A fraudulent EV certificate for `api.pcloud.com` would be accepted. Operators with stricter requirements can run the daemon behind a pinning proxy. The release-signing EV key is held offline in an HSM; the revocation path is documented in `docs/book/src/ops/incident.md`.

## Incident Response Handles

When something looks wrong, operators have three concrete handles:

- **`pcloudc doctor`** — prints vault permissions, active endpoint, TLS posture, audit-chain integrity, plugin signatures, clock skew, and FUSE status. Exits non-zero on any failure so it can wire into `systemd` health probes. First stop for "is my install healthy".
- **`/slo` endpoint** — the daemon's local HTTP endpoint exposes per-subsystem status, backpressure depth, last-successful-sync timestamps, audit persistence status, TLS handshake counters, and recent error rates. Wire it into your monitoring.
- **Hash-chained audit log** — verify the chain with `pcloudc audit verify`; forward to a SIEM for tamper-evidence beyond the local host.

If any of these report a problem, stop the daemon, capture the audit log and the vault metadata (not the vault contents), and file an issue with the output. The maintainers will not ask you for your password or token.

# Security Operations

This chapter is the **operator-facing** version of the security rules
encoded in `CLAUDE.md` and the architecture-level [Security Model](../security/model.md).
It documents what the daemon enforces, what the operator must do, and
what an inspection or audit looks like in practice.

> **Honesty callout.** The Rust rewrite is deliberately stricter than
> the legacy C client on every dimension below. Where the legacy
> behaviour conflicts with normal enterprise security expectations,
> the Rust path keeps the secure default and the legacy behaviour is
> documented as intentionally dropped. See ADRs 0004, 0005, 0007,
> 0015, 0016.

---

## 1. Secret discipline

### 1.1 What is a secret in this codebase

| Secret | Lifetime in memory | Persisted on disk? |
|--------|--------------------|--------------------|
| pCloud account password | Lives only inside one `SecretString` for the duration of the auth call. | **Never.** |
| OAuth / pCloud auth token | Held in `SecretString` inside the runtime. | **Opt-in only**, encrypted vault, mode `0600` (Unix) / SID-locked (Windows). |
| TFA code | `SecretString`, dropped after submission. | Never. |
| Crypto password | `SecretString`, dropped after key derivation. | Never (ADR 0007). |
| Wrapped crypto master key | `SecretBytes`. | Yes — wrapped with a KDF-derived key, never written in clear. |
| Recovery codes | `SecretString` per code. | Never, by design. The user is responsible for offline storage. |

All `SecretString` and `SecretBytes` wrappers:

- redact in `Debug` / `Display` output,
- zeroize on `Drop`,
- refuse `Clone` unless the call site explicitly opts in.

### 1.2 What you must not do

- Do **not** export the vault to a tarball, S3 bucket, or fleet
  manager. Tokens are bound to the daemon UID; they will not unwrap
  on a different host.
- Do **not** pipe the password through environment variables in a
  shell script that is committed or kept in shell history.
- Do **not** redirect `pcloud-cli login --password ...` output to a
  log; the CLI strips the password from its own argv after parsing,
  but the shell may have already captured it.
- Do **not** restore a vault from a backup taken before a forced
  password change. The wrapped tokens will be valid but will fail at
  the API; clearer error messages come from a fresh login.

### 1.3 What you may do

- Back up the **state directory** (SQLite store + journal). It does
  not contain plaintext secrets.
- Audit-log the `pcloud-cli` invocation history through your shell's
  audit subsystem (`auditd` etc.) — the CLI never echoes secrets to
  argv after parsing.
- Run the daemon under `DynamicUser=yes` (systemd) or an analogous
  ephemeral user. The vault path follows the dynamic UID's home; the
  daemon does not require a fixed account.

---

## 2. Vault location and posture

### 2.1 Default paths

| Platform | Vault directory | Mode |
|----------|-----------------|------|
| Linux | `${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/` | `0700` dir, `0600` file |
| macOS | `~/Library/Application Support/pcloud-rs/` | `0700` dir, `0600` file |
| Windows | `%LOCALAPPDATA%\pcloud-rs\` | NTFS DACL, user SID full control, inheritance disabled |
| FreeBSD / OpenBSD / NetBSD | `${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/` | `0700` dir, `0600` file |

### 2.2 What the daemon refuses

- Vault file with mode other than `0600` on Unix → daemon refuses
  to open and emits `vault: refusing to open: mode=<m> expected=0600`.
- Vault file owned by a UID other than the daemon UID → daemon
  refuses to open.
- Parent directory mode wider than `0700` on Unix → daemon refuses.
- On Windows, a vault directory whose DACL does not match the
  user's SID-only ACE list → daemon emits `vault: ACL drift
  detected` and refuses to open.

The daemon **does not** silently chmod or chown drifted state. This
is intentional: silent repair masks tampering. Repair manually after
investigating root cause (see [Troubleshooting § 7](./troubleshooting.md#7-permission-errors-on-socket--vault--mount)).

### 2.3 Forced rotation

To rotate the vault (e.g. after a personnel change on a shared host):

```bash
systemctl --user stop pcloudd.service
shred -u "${XDG_DATA_HOME:-$HOME/.local/share}/pcloud-rs/auth-vault.json"
systemctl --user start pcloudd.service
pcloud-cli login
```

Wrapped tokens not in the vault are unrecoverable. There is no
"backup vault" path by design.

---

## 3. IPC peer-credential enforcement

### 3.1 Unix — `SO_PEERCRED`

Every accepted IPC connection on Linux, FreeBSD, OpenBSD, NetBSD, and
macOS is authenticated with the kernel's peer-credential mechanism
(`SO_PEERCRED` on Linux/BSD, `LOCAL_PEERPID` + `LOCAL_PEEREUID` on
macOS). The accept loop rejects any peer whose effective UID does
not match the daemon UID. There is no remote network listener; the
socket is bound under a `0700` runtime directory.

What this means operationally:

- A second user account on the same host cannot call the daemon's
  IPC, even with the socket path in hand.
- A `setuid` binary cannot bypass peer-cred — the kernel reports the
  effective UID at the time of `connect()`.
- Container deployments that share the host UID namespace (rootless
  containers) work transparently; namespaced UID containers must
  share the daemon's UID, or they will be rejected.

### 3.2 Windows — named-pipe DACL + `GetNamedPipeClientProcessId`

> **Tier-2 status (2026-04-24).** The Windows named-pipe accept loop
> is in flight — `pcloudd-svc` currently runs a no-op stub that
> compiles and starts but does not yet serve IPC. See `CLAUDE.md`
> § "Windows posture" for the gating remaining work.

When the accept loop lands:

- The named pipe is created with an explicit DACL granting access
  only to the current user's SID.
- Each connect-side handle is queried via
  `GetNamedPipeClientProcessId` and the resulting token's user SID
  is compared against the daemon's user SID before any request is
  dispatched.
- Connections from other SIDs are dropped at accept time without
  ever reading a request body.

### 3.3 Audit log integration

Every accept failure (peer-UID mismatch, malformed framing, slow
client) emits an audit event with:

- the accept timestamp,
- the rejected peer UID (Unix) or SID (Windows),
- the failure category,
- a hash-chained pointer to the previous audit event.

The audit log is append-only and tamper-evident. Inspect with:

```bash
pcloud-cli audit tail --since '1 hour ago'
pcloud-cli audit verify
```

`audit verify` walks the hash chain and reports the byte offset of
the first break, if any. A break is a strong signal of tampering or
disk corruption — investigate before trusting the daemon further.

---

## 4. TLS-only in production

### 4.1 What the daemon enforces

- Production builds **reject** any configuration that disables TLS.
  The transport constructor fails closed at build time; there is
  no `--insecure` flag.
- Server-name verification is mandatory. The TLS stack uses
  `rustls-native-certs` to load the system trust store; expired
  roots fail closed.
- The daemon validates the API endpoint hostname against an
  allow-list of pCloud regional endpoints
  (`api.pcloud.com`, `eapi.pcloud.com`, plus mirrors documented in
  the published pCloud API reference). Endpoints not in the
  allow-list are rejected.
- HSTS-style cache: once an endpoint succeeds with TLS, the daemon
  refuses to re-attempt the same endpoint on a downgraded transport
  for the lifetime of the process.

### 4.2 What the operator must do

- Keep the system trust store updated. `update-ca-certificates`
  (Debian/Ubuntu), `update-ca-trust` (Fedora/RHEL), `softwareupdate`
  (macOS), Windows Update.
- If a corporate MITM proxy injects its own CA, install that CA in
  the system trust store. The daemon honours the system store; it
  does **not** maintain its own CA bundle.
- Do **not** run the daemon under `SSL_CERT_FILE=/dev/null` or any
  similar override. The daemon ignores environment variables that
  point at empty trust stores in production builds.

### 4.3 Suspected MITM

See [Troubleshooting § 4](./troubleshooting.md#4-tls-pinning-mismatch--certificate-errors).
Short version: capture the live certificate fingerprint from a
known-good network, compare against the suspect network. If they
differ, stop the daemon and treat the network as hostile.

---

## 5. Audit-log inspection

### 5.1 What is logged

- Every authentication outcome (success, failure, TFA challenge).
- Every IPC accept (peer UID/SID, success or rejection reason).
- Every persistence write (vault, store, journal) with byte length.
- Every mount/unmount.
- Every config change.

### 5.2 What is **not** logged

- Secrets, in any form.
- Full request/response bodies.
- File contents or path components below the sync-root, by default.

### 5.3 Tamper detection

The audit log is a single append-only file. Each record carries:

- monotonic sequence number,
- timestamp,
- a SHA-256 hash that covers the previous record's hash + the
  current record's body.

Tampering breaks the hash chain. `pcloud-cli audit verify` reports
the first break. The chain is **not** signed — tampering by an
attacker with full write access to the log file cannot be ruled out
by this mechanism alone. Combine with filesystem-level immutability
(append-only `chattr +a`, ZFS snapshots, WORM storage) for
defence-in-depth.

### 5.4 Forwarding to a SIEM

The daemon emits structured JSON logs to stderr (and via journald
when run under systemd). Forward to your SIEM via:

- journald → `systemd-journal-remote` → Splunk/Elastic.
- Filebeat / Vector / Fluent Bit reading the per-host log file
  produced under `LogsDirectory=`.
- The OTel exporter, if enabled — see
  [`reference/config.md`](../reference/config.md).

---

## 6. Hardening checklist

Run through this before declaring a deployment "operationally
acceptable" (this is **not** a "production-ready" claim — see
[`STATUS.md`](https://github.com/ezechiel203/pcloud-rs/blob/main/STATUS.md)).

- [ ] systemd unit installed from `packaging/systemd/pcloudd.service`.
- [ ] `DynamicUser=yes` or a dedicated low-privilege fixed user.
- [ ] FUSE drop-in installed only if mounts are required
      (`packaging/systemd/override-fuse.conf.example`).
- [ ] `IPAddressAllow=` widened to the pCloud API endpoints in use.
- [ ] Vault `0600`, parent dir `0700`, owner = daemon UID.
- [ ] IPC socket `0600`, runtime dir `0700`.
- [ ] System trust store current.
- [ ] Audit log forwarding to SIEM verified by injecting a known
      event and confirming arrival.
- [ ] `pcloud-cli audit verify` returns clean.
- [ ] Backup of the state dir excludes the vault file.
- [ ] Crypto password documented in your password manager (it is
      **not** recoverable via pCloud).

---

## 7. Cross-references

- Architecture: [`security/model.md`](../security/model.md)
- Threat model: [`security/threat-model.md`](../security/threat-model.md)
- Secrets handling: [`security/secrets.md`](../security/secrets.md)
- Audit dossier: [`security/audit-dossier.md`](../security/audit-dossier.md)
- ADR 0004 — Panic guard default-on: [`adr/0004.md`](../adr/0004.md)
- ADR 0005 — Token vault layout: [`adr/0005.md`](../adr/0005.md)
- ADR 0007 — Crypto password not persisted: [`adr/0007.md`](../adr/0007.md)
- ADR 0015 — Vault `0600` enforcement: [`adr/0015-vault-0600-permission-enforcement.md`](../adr/0015-vault-0600-permission-enforcement.md)
- ADR 0016 — Secret-wrapping discipline: [`adr/0016-secret-wrapping-discipline.md`](../adr/0016-secret-wrapping-discipline.md)

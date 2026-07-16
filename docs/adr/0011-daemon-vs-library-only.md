# ADR 0011: Daemon Architecture vs Library-Only SDK

- Status: Accepted
- Date: 2026-04-16

## Context

The legacy `pcloud-rs` C client is a single-process binary: the CLI, the
sync engine, the mounted-drive adapter, and the pCloud API client all
share one address space. That shape is simple but has a cost:

- every consumer (CLI, future GUI, web UI, SDK embedders) must embed the
  full sync engine and mount runtime;
- auth state, crypto state, and vault material sit in whichever process
  happened to run last, with no single owner;
- cross-tool UX (e.g. `pcloudc status` while the sync engine runs) has
  no clean path and the C client historically relied on ad-hoc PID
  files and stdio signalling;
- there is no boundary at which to enforce per-caller authentication,
  rate limiting, audit, or policy.

At the same time, an SDK-only crate has its own problems: consumers end
up with divergent long-running daemons, no shared audit chain, no
single crypto unlock lifetime, and no place to attach enterprise
concerns (fleet enrolment, OIDC broker, policy evaluation).

## Decision

The Rust rewrite ships a **long-lived daemon (`pcloudd`) plus thin
clients** architecture:

1. `pcloudd` owns all state: auth vault, crypto state, sync roots,
   upload/download journals, page cache, mount lifecycle, and the audit
   chain.
2. Every consumer (CLI `pcloudc`, web UI `pcloud-web`, the SDK crate
   `pcloud-sdk`, first-party plugins) talks to the daemon over the
   owner-only local IPC described in ADR 0002.
3. `pcloud-sdk` is an **ergonomic wrapper over the IPC client**, not a
   reimplementation of the engine. Its 1.x contract exposes only SDK-owned
   remote-drive types. The historical in-process path is isolated as the
   unpublished, evolving `pcloud-embedded-sdk` compatibility crate.
4. Enterprise concerns (`pcloud-idp`, `pcloud-policy`, `pcloud-fleet`,
   `pcloud-kms`, `pcloud-session`) attach at the daemon boundary, not
   per-client.

## Consequences

Good:

- Single owner for secrets: the auth vault is only opened once, by the
  daemon, under `0600` perms (ADR 0005, ADR 0007); clients never see
  raw tokens.
- Single audit chain: every command goes through `dispatch.rs` and is
  traceable end-to-end with OTel (see ADR 0012).
- Enterprise extensions ride the IPC envelope without touching every
  client crate.
- Crash blast radius is contained: a CLI panic cannot corrupt the store
  or the mount; the dispatch boundary catches unwinds (ADR 0004).
- The SDK can evolve at a different cadence from the daemon wire
  format; `try_from_wire` handles backward-compat (see ADR 0012).

Bad:

- Two processes to reason about; operators must understand daemon
  lifecycle. Mitigated by `pcloudc doctor`, systemd/launchd units, and the
  per-user Windows launcher. The public Windows package deliberately installs
  no SCM service because named-pipe, DPAPI, and WinFSP ownership must share the
  interactive user's SID.
- IPC adds latency vs in-process calls (sub-millisecond on loopback,
  acceptable for every retained workload).
- Cross-platform IPC surface (AF_UNIX on Unix, named pipe on Windows)
  needs per-platform peer-auth logic. Covered by the cross-platform
  wave and `pcloud-ipc` crate.

## Alternatives Considered

- **Library-only SDK (no daemon)**: rejected — forces every embedder
  to own the sync engine, duplicates crypto state, and leaves enterprise
  concerns with nowhere to attach. Also blocks a shared audit chain.
- **Single-binary C-style monolith**: rejected — conflicts with the
  security and containment goals stated in `CLAUDE.md`, and leaves the
  CLI unable to query a running sync engine without process-wide locks.
- **RPC over HTTP on loopback**: considered; rejected in ADR 0002 in
  favour of binary length-prefixed frames. The daemon boundary itself
  is unaffected by that choice.

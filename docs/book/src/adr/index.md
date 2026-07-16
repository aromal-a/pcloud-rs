# Architecture Decision Records

This chapter mirrors
[`docs/adr/README.md`](https://github.com/ezechiel203/pcloud-rs/tree/main/docs/adr)
inside the mdBook tree so readers can hop between ADRs 0001–0020
without leaving the book. ADR **source files** live in
`docs/adr/*.md` and the pages under this chapter include
their bodies verbatim via the mdBook `{{#include}}` directive — this
chapter's stub files must therefore never be edited to drift from
the sources.

Each ADR captures a single non-trivial decision, its rationale, and
its alternatives. The format itself is defined in ADR 0001.

Status values in use: `Accepted`, `Proposed`, `Superseded by ADR NNNN`,
`Deprecated`. Never rewrite an ADR in place to change the decision;
write a new one and supersede the old.

## Index

- [0001 — Record Format](./0001.md) — Accepted. Defines the template,
  file-naming rules, and status lifecycle for all ADRs.
- [0002 — IPC Socket Framing](./0002.md) — Accepted. Binary
  length-prefixed frames over `AF_UNIX`, not JSON-over-HTTP.
- [0003 — Sync Mutex Choice](./0003.md) — Accepted.
  `parking_lot::Mutex`/`RwLock` workspace-wide: poison-free and faster
  on the uncontended path.
- [0004 — Panic Guard Default-On](./0004.md) — Accepted.
  `catch_unwind` at the IPC dispatch boundary is unconditional;
  documents what is and is not caught.
- [0005 — Token Vault Layout](./0005.md) — Accepted. `0600` file under
  a `0700` directory, atomic writes, and opt-in durability via
  `PCLOUD_DURABLE_AUTH_TOKENS=1`.
- [0006 — No Update Check](./0006.md) — Accepted. The rewrite
  deliberately does not mirror `psync_check_new_version*`; distro
  channels own updates.
- [0007 — Crypto Password Not Persisted](./0007.md) — Accepted. A
  retained-security carve-out against the C behaviour: crypto
  passwords live only in memory.
- [0008 — Streaming Download Buffer Size](./0008.md) — Accepted. 64
  KiB buffer on the streaming copy loop, justified against syscall
  count, memory footprint, and TLS record size.
- [0009 — Parity Matrix Truth Source](./0009.md) — Accepted.
  `STATUS.md` is authoritative for aggregate parity claims;
  `CLAUDE.md` and `README.md` are consumers.
- [0010 — FUSE Write-Path Daemon Wiring Pending](./0010.md) —
  Superseded by ADR 0020. Records the earlier open mounted-drive write
  questions.
- [0011 — Daemon vs Library-Only](./0011.md) — Accepted. Documents
  why the project ships a long-lived daemon plus thin CLI rather than
  a library-only crate.
- [0012 — Traceparent Envelope Wrapper](./0012.md) — Accepted. W3C
  Trace Context propagation across the IPC boundary.
- [0013 — OPA Rego via Regorus](./0013.md) — Accepted. Embedded
  Rego policy evaluator chosen over a sidecar OPA process.
- [0014 — Hand-Rolled OIDC Broker](./0014.md) — Accepted. Why the
  enterprise OIDC broker is in-tree rather than a vendored crate.
- [0015 — Vault 0600 Permission Enforcement](./0015.md) — Accepted.
  Refusing to start the daemon if the vault file is not 0600/0700.
- [0016 — Secret-Wrapping Discipline](./0016.md) — Accepted. The
  `SecretString`/`SecretBytes` rule applied workspace-wide.
- [0017 — JSON in Message Response Shape](./0017.md) — Accepted.
  Why responses carry JSON bodies inside the framed envelope.
- [0018 — Native Field-Selector Syntax](./0018.md) — Accepted.
  Field-selector grammar for partial-response queries.
- [0019 — IPC Serve Loop Is Single-Threaded](./0019.md) — Accepted.
  Documents why the production accept loop handles one request at a
  time (`RuntimeShell` is intentionally `!Send`), what timeouts bound
  worst-case latency, and when the decision should be reopened.
  Audit finding: `audit-06 §7-sonnet M2` / `pcloud-rs-ncx.56`.
- [0020 — FUSE Write Durability and Bounded Staging](./0020.md) — Accepted.
  `fsync` is server-durable, staging ceilings return `ENOSPC`, and both
  FUSE compositions share one resumable write service.

## How to Add a New ADR

1. Copy the template described in ADR 0001 into
   `docs/adr/NNNN-slug.md` with the next number.
2. Add a stub `docs/book/src/adr/NNNN.md` whose only content is a
   mdBook include directive pointing to `../../../adr/NNNN-slug.md`.
3. Add an entry to `docs/book/src/SUMMARY.md` under this chapter.
4. Add an entry to this index and to `docs/adr/README.md`.

Never edit the include-stub to add prose — the ADR source is the
single writable copy.

# Architecture Decision Records

This chapter mirrors
[`docs/adr/README.md`](https://github.com/pcloudcom/pcloud-rs/tree/main/docs/adr)
inside the mdBook tree so readers can hop between ADRs 0001–0010
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
  Proposed. Records the shape of the remaining mounted-drive write
  wiring (`bd-1du.4.6`) and the two open sub-decisions (back-pressure
  policy and `fsync` durability guarantee).

## How to Add a New ADR

1. Copy the template described in ADR 0001 into
   `docs/adr/NNNN-slug.md` with the next number.
2. Add a stub `docs/book/src/adr/NNNN.md` whose only content is a
   `{{#include ../../../adr/NNNN-slug.md}}` line.
3. Add an entry to `docs/book/src/SUMMARY.md` under this chapter.
4. Add an entry to this index and to `docs/adr/README.md`.

Never edit the include-stub to add prose — the ADR source is the
single writable copy.

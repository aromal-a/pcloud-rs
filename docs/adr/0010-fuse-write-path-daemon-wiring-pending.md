# ADR 0010: FUSE Write-Path Daemon Wiring Pending (`bd-1du.4.6`)

- Status: Superseded by ADR 0020
- Date: 2026-04-15

ADR 0020 records the implemented decision: writes are journal-first, `fsync`
is server-durable, staging is bounded with `ENOSPC`, chunked uploads resume by
acknowledged offset, and both concrete and object-safe FUSE compositions use
the same write service.

## Context

The `pcloud-fs` crate has, over the P0–P2 phases, grown most of the
pieces needed for a real mounted-drive experience:

- Linux FUSE mount/unmount scaffolding with RAII handles and
  signal-aware unmount cleanup.
- Policy validation on mount targets (ownership, mode, nesting against
  sync roots).
- An in-memory read path wired to the cache and to signed download
  URLs.
- A staging area for writes, with a crash-safe write-ahead journal.
- Writeback helpers that can drain the staging area back to the API.

What is **not** yet wired end-to-end is the bridge between the FUSE
write operations (`write`, `flush`, `fsync`, `release`) and the
daemon's runtime upload pipeline. Today the write path lands in the
staging journal but does not schedule a real upload against the
transfer backend under the supervision of the runtime; it is exercised
only by unit tests against a mock backend.

This is the remaining work tracked under bead **`bd-1du.4.6`**, and it
is the reason parity on "Filesystem / mounted drive" still shows as a
live open area in `STATUS.md`.

The reason this ADR is **Proposed** rather than **Accepted** is that
there is genuine open design choice on two points, neither of which has
been ratified yet:

1. **Back-pressure policy.** Should `write(2)` block when the staging
   journal is full, return `ENOSPC`, or silently accept and queue?
   Each has a different failure mode for interactive editors and for
   large copies. Reviewer input so far has been split.
2. **Crash-window guarantee at `fsync`.** The minimum we will commit
   to is "the byte is durable in the journal". Some reviewers want
   "the byte is durable on the server" before `fsync` returns, which
   has very different latency characteristics.

## Decision

This ADR records the *shape* of the decision and the open
sub-decisions. The wiring is intentionally not yet merged.

Proposed direction, pending ratification:

- `write` buffers into the staging journal and returns as soon as the
  journal fsync completes. This preserves POSIX semantics locally.
- `flush` / `release` enqueue an upload against the runtime transfer
  pipeline. Errors from the pipeline surface on the next `fsync` or
  `close`, consistent with how network filesystems already behave.
- `fsync` semantics default to **journal-durable** (the proposed
  default), with a mount option to upgrade to **server-durable** for
  workloads that require it. This option will be specified in the
  follow-up ADR that ratifies this one.
- Back-pressure: `write` blocks once the staging journal exceeds a
  configurable high-water mark, returning `EINTR` on signal and
  `ENOSPC` only when the local staging filesystem itself is out of
  space.

## Consequences

If accepted as proposed:

Good:

- POSIX semantics preserved for common editor and tooling workflows.
- Crash-window is bounded by journal fsync, which is the standard
  guarantee for modern local filesystems.
- Clear upgrade path (mount option) for workloads needing
  server-durable fsync.

Bad:

- Journal-durable fsync can surprise operators who expect "the file
  is on the server when fsync returned". Must be documented loudly
  in the mount manual and in `OPERATIONS-RUNBOOK.md`.
- Adds a real supervised background task (the writeback worker),
  which means the panic-supervision story in ADR 0004 must be
  extended with explicit restart/backoff policy for this task.

Until this ADR is moved to **Accepted** and the code is merged:

- `STATUS.md` continues to show filesystem parity as an open area.
- The parity matrix rows for mounted-drive writes stay `Partial`.
- No release wording may claim "mounted-drive parity" or
  "drop-in replacement for the C client's mount" (see ADR 0009).

## Alternatives Considered

- **Synchronous server-durable fsync as default**: rejected for the
  default; accepted as a proposed opt-in mount option. Too slow for
  interactive workloads.
- **No staging journal; upload directly from write**: rejected —
  destroys POSIX semantics and makes every write's latency equal to
  HTTP round-trip time.
- **Defer the whole write path; ship read-only mount**: considered
  seriously. May yet become the interim release shape if the
  sub-decisions above do not converge quickly; a new ADR would
  record that fallback and supersede this one.

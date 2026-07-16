# ADR 0020: FUSE Write Durability and Bounded Staging

- Status: Accepted
- Date: 2026-07-16
- Supersedes: ADR 0010

## Context

ADR 0010 left two material choices open: what happens when local staging is
full, and whether a successful `fsync(2)` means only journal durability or a
completed remote commit. The write path, canonical `RemoteFs` transfer bridge,
and real-kernel tests have now converged on one contract. Leaving ADR 0010 as
the apparent current decision made the documentation contradict the code.

The mount has two composition paths: the daemon's concrete `PcloudFsShim` and
the object-safe `FuserShim<A>` adapter used at the platform boundary. They must
not have different correctness or durability semantics.

## Decision

1. `WritePathService` is the sole owner of staged write state. A write record
   is appended and fsynced before the staging blob is mutated. Sparse writes
   zero-fill gaps and publish the authoritative staging length immediately to
   kernel-visible metadata.
2. Per-inode and process-wide staging ceilings bound disk exposure. A write
   that would exceed either ceiling fails before mutation with `ENOSPC`.
   Invalid paths and names continue to use `EINVAL`.
3. `flush` and `fsync` append a durable flush barrier, complete the remote
   upload, and only then checkpoint records for that path. A successful
   `fsync` is therefore **server-durable**, not merely journal-durable. Upload
   failure is returned to the kernel and a failed drain is never summarized as
   success.
4. Threshold-triggered large writes use `upload_create`, bounded
   `upload_write` chunks, and `upload_save`. Every acknowledged offset is
   persisted. Transient failures replay the unacknowledged chunk; stale or
   permanently failed sessions restart once from offset zero and otherwise
   surface an error.
5. The generic adapter carries an object-safe write delegate. When a writer is
   attached, create/write/flush/fsync/truncate/unlink/rename use the same
   `WritePathService` as `PcloudFsShim`; without one they return `ENOSYS`.
6. Clean upload completion checkpoints the journal. Replay after a clean
   `fsync` is empty; crash replay contains only work that was not remotely
   acknowledged.

## Evidence

- `write_path` unit tests cover journal-before-mutation ordering, retry,
  conflict/session restart, sparse growth, staging ceilings, checkpointing,
  and crash replay.
- `scripts/linux-release-mount-gate.sh` runs all practical real-kernel Linux
  mount tests serially, then the independent 2 GiB transient-retry stress test,
  and rejects leaked mounts.
- `fuse_dyn_shim_write` proves the object-safe adapter publishes size and
  returns byte-exact VFS readback.
- `fuse_kernel_e2e` proves 64 MiB create/write/fsync/read/rename/unlink through
  a real kernel mount.
- `fuse_write_path_live` proves immediate readback and cold-remount byte
  identity using fresh staging and journal state.

These are deterministic/local-backend proofs. A credentialed pCloud run and
native macOS, Windows, and BSD release-commit results remain platform
qualification gates, not unresolved semantics in this ADR.

## Consequences

### Positive

- Applications receive meaningful `ENOSPC` capacity failures.
- `fsync` has one strong and testable meaning across mount compositions.
- Journal replay cannot resurrect already acknowledged writes.
- Large writes use bounded memory and retry without duplicating acknowledged
  chunks.

### Negative

- `fsync` latency includes the remote commit and therefore depends on network
  health. This is intentional: returning earlier would misrepresent
  durability.
- Offline writers eventually fail rather than accumulating unbounded local
  data.
- The 2 GiB test requires temporary disk capacity and is restricted to release
  qualification rather than ordinary developer tests.

## Alternatives Considered

- **Journal-durable `fsync` with background upload:** rejected because callers
  cannot distinguish local intent from remote durability and unmount/drain can
  fail after an apparent successful sync.
- **Unbounded staging:** rejected because one or many writers could exhaust the
  host filesystem.
- **Different concrete and object-safe write paths:** rejected because platform
  selection must not change filesystem correctness.
- **Direct upload from each `write(2)`:** rejected because network RTT would
  dominate small writes and partial failure would be difficult to replay
  idempotently.

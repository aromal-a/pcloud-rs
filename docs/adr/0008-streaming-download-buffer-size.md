# ADR 0008: Streaming Download Buffer Size = 64 KiB

- Status: Accepted
- Date: 2026-04-15

## Context

The P1.5 phase converted the HTTP download path from a read-all-into-a-
`Vec<u8>` model to a streaming copy into the destination file. That
change forced a decision about the per-iteration buffer size: the
reader reads into a stack-ish buffer, hashes incrementally, and writes
to the sink.

Inputs considered:

- **Typical TCP segment sizes**: 1.46 KiB MSS on commodity Linux, modulo
  offloading. Anything below ~16 KiB leaves throughput on the table
  because the hot loop becomes syscall-bound rather than throughput-
  bound.
- **Page-cache alignment**: 4 KiB page, 64 KiB is a clean multiple and a
  commonly-used unit for `read` / `write` syscalls to minimise
  per-syscall overhead without blowing out the L1/L2 footprint of the
  copy loop.
- **Memory footprint**: the daemon may run many concurrent downloads.
  A 64 KiB buffer per stream means 1000 concurrent downloads cost
  ~64 MiB in stream buffers — tolerable; 1 MiB buffers would cost
  ~1 GiB and break small-footprint deployments.
- **Hashing throughput**: SHA-256 / BLAKE3 throughput on contemporary
  CPUs plateaus well below 64 KiB block sizes; going larger buys
  nothing for the checksum path.
- **TLS record size**: modern TLS implementations hand back records of
  up to 16 KiB; 64 KiB comfortably absorbs one or more records per
  iteration.

## Decision

Streaming downloads use a **64 KiB** buffer on the read/hash/write loop.
The constant lives in one place (`pcloud-proto::transfer_api`
streaming module) as `const STREAM_CHUNK: usize = 64 * 1024;` and is
used unchanged by the write-side of the same crate for symmetry.

## Consequences

Good:

- Syscall count on a 1 GiB download drops to ~16 384 read/write pairs
  regardless of HTTP record boundaries, low enough that the loop is
  dominated by actual I/O rather than syscall overhead.
- Memory cost scales linearly with concurrency and is bounded at
  `64 KiB × in_flight_streams`.
- Single named constant is the right shape for a future
  configuration hook if benchmarking ever justifies making it
  tunable.

Bad:

- Not optimal on every workload. Very small files pay the full 64 KiB
  buffer allocation even if the entire body is 2 KiB. Acceptable: the
  buffer is stack- or arena-backed, not per-request-heap-allocated on
  the hot path, and 64 KiB is within the stack budget we already
  require for normal tokio tasks.
- Not optimal on extremely high-throughput LAN links where a 1 MiB
  buffer would shave a few percent. Out of scope for this fork's
  target deployments.

## Alternatives Considered

- **8 KiB** (the `std::io::copy` historical default): rejected —
  syscall-heavy, measurably slower on gigabit links in our microbench.
- **16 KiB**: considered; roughly matches TLS record size. Chose
  64 KiB for the syscall-count and cache-friendliness reasons above.
- **1 MiB**: rejected — memory footprint under concurrency is
  disproportionate to the throughput gain.
- **Dynamically tuned buffer**: rejected — complexity not justified by
  benchmark; revisit if a real workload proves the fixed size
  wasteful. A future ADR would supersede this one if so.
- **Memory-mapped writes to the destination file**: rejected —
  complicates partial-file rollback on error, and does not compose
  with the incremental hashing step that must see every byte
  exactly once.

# Performance

This chapter documents the performance wins landed in **wave-1** of the Rust
rewrite's optimisation pass. Each win is traceable to a plan item ID (`P1.1`,
`P5.1`, `P1.5`, `P5.2` / `G8`, `C1`, `C4`). Numbers come from Criterion
micro-benchmarks checked in under each owning crate's `benches/` directory
and from `cargo test --release` soak runs. There is no aggregate
`pcloud-bench` crate today. Where a number is absent, the bench is still
warming up a baseline and the section is marked
*(bench pending baseline)*.

All paths are relative to `` unless otherwise stated.

## Summary table

```
+--------------------------------+----------------+-------------------+---------------+
| Win (plan id)                  | Before         | After             | Improvement   |
+--------------------------------+----------------+-------------------+---------------+
| P1.1  Page-cache eviction      | O(n) LRU scan  | O(1) intrusive    | ~180x @ 10k   |
| P5.1  Arc<Vec<u8>> hot-path    | Vec clone/hit  | Arc clone/hit     | ~1000x (hit)  |
| P1.5  Streaming HTTP download  | Full-body buf  | 64 KiB window +   | peak RSS      |
|                                | in RAM         | rolling SHA256    | flat w/ size  |
| P5.2/G8 Chunked write-flush    | Monolithic     | Bounded chunks    | ~3.4x @ 128M  |
|                                | flush()        | + back-pressure   |               |
| C1   flush_latency_seconds     | no visibility  | Prom. histogram   | observability |
+--------------------------------+----------------+-------------------+---------------+
```

Reproduce with:

```bash
cargo bench -p pcloud-fs --bench chunked_flush
cargo bench -p pcloud-fs --bench page_cache
cargo bench -p pcloud-embedded-sdk --bench upload_session
```

Criterion writes summaries under `target/criterion/`. No benchmark workflow
exists today, so release managers must run these manually until a CI gate is
added.

## P1.1 — O(1) page-cache eviction

### What was slow

The page cache used in `pcloud-fs` for read-path buffering originally kept its
LRU ordering as a `VecDeque<CacheKey>` scanned linearly on every eviction.
Under hot random-read workloads (FUSE `read` callbacks at > 5 k IOPS) the
cache walked tens of thousands of entries per eviction, and the scan showed
up at the top of a `perf record` flamegraph as a single monomorphised
`Vec::retain` symbol burning > 40 % of the daemon's CPU time.

### What is fast now

The cache now uses an intrusive doubly-linked list keyed by `u64` entry
handle, backed by a `HashMap<CacheKey, EntryHandle>`. Promotion to MRU,
insertion, and eviction of the LRU tail are all **O(1)** with two pointer
updates. No scan, no allocation on the hot path.

### Numbers

```
page_cache_evict/10k_entries    time:   [5.41 µs 5.48 µs 5.56 µs]
page_cache_evict/10k_entries    (prior) [~980 µs 1.01 ms 1.04 ms]
```

That is roughly a **180× speed-up** on the 10 k-entry eviction microbench and
eliminates the hot-path scan entirely. Worst-case latency under a synthetic
64 k-entry cache went from *multi-millisecond* tail spikes to flat
single-digit microseconds.

See `crates/pcloud-fs/src/page_cache.rs` and
`crates/pcloud-fs/benches/page_cache.rs`.

## P5.1 — `Arc<Vec<u8>>` hot-path

### What was slow

Cached file pages were stored as `Vec<u8>` and *cloned* on every cache hit,
because the read path wanted an owned buffer it could hand to FUSE. On a
4 MiB page that is a 4 MiB `memcpy` per hit. Cache hits are the common case,
so every cloned page was paid again and again.

### What is fast now

The cache now stores `Arc<Vec<u8>>`. A hit returns `Arc::clone(&entry)`,
which is a single atomic refcount bump — independent of page size. The FUSE
adapter reads through the `Arc` without copying; the buffer is only freed
when the last reader drops its reference.

### Cache-hit semantics

```
Hit (4 MiB page):
  Vec<u8> clone      ~= 900 µs   (memcpy 4 MiB)
  Arc<Vec<u8>> clone ~= 0.9 µs   (atomic fetch_add)
```

That is the **~3 orders of magnitude** figure we quote for cache-hit cost.
A sustained read-heavy workload with 90 % cache-hit rate shows ~6× lower
daemon CPU time end-to-end.

Correctness note: `Arc<Vec<u8>>` is immutable-by-construction once inserted.
Write-path invalidation removes the `Arc` entry and inserts a fresh one;
in-flight readers keep their old `Arc` alive until their read completes, so
there is no tearing.

See `crates/pcloud-fs/src/page_cache.rs`.

## P1.5 — Streaming HTTP download

### What was slow

The transfer backend's download path previously read the entire HTTP
response body into a single `Vec<u8>` before hashing and writing. For a
1 GiB download that meant the daemon's RSS briefly spiked by 1 GiB while
also holding the destination `File` open. Large transfers OOM'd on small
hosts.

### What is fast now

The download loop now pulls a **64 KiB window** from the TLS stream, feeds
it into a rolling `sha2::Sha256` state, and writes it to the destination
file — all without ever materialising the full body. The 64 KiB choice is
recorded in [ADR-0008](../adr/0008.md); it balances syscall overhead against
peak RSS.

### Result

- Peak RSS during download is flat in file size: a 10 GiB download now peaks
  at the same ~2 MiB of buffer as a 10 MiB download.
- End-to-end throughput is unchanged on a fast link (TLS-bound, not
  buffer-bound) and *better* on slow links, because the file fsync interval
  is decoupled from the body size.
- SHA256 verification is folded into the stream, so an integrity failure is
  caught at the last byte without a second pass.

See `crates/pcloud-proto/src/transfer_api.rs` (the `download_stream` helper)
and ADR-0008.

## P5.2 / G8 — Chunked write-path flush

### What was slow

`WritePathService::flush` used to call `flush_all` in a single synchronous
call that walked every dirty page in the write-through journal. On a
128 MiB pending flush the call blocked the FUSE `flush` callback for
hundreds of milliseconds, and a panic mid-flush left the journal in a
half-applied state.

### What is fast now

Flush is now chunked at `WRITE_FLUSH_CHUNK = 1 MiB` (tunable). Each chunk:

1. Drains one page range from the journal,
2. Issues the `upload_write` call,
3. Updates the journal head,
4. Yields the scheduler.

Back-pressure comes from a semaphore (`max_in_flight_chunks = 4`) so the
network side never queues unbounded work. A crash mid-flush resumes at the
last durable chunk, not from scratch.

### Numbers

```
chunked_flush/128MiB       time:   [1.82 s 1.87 s 1.93 s]
chunked_flush/128MiB       (prior) [6.10 s 6.35 s 6.61 s]
```

About **3.4× faster** on a 128 MiB flush with the default concurrency, and
worst-case `flush` callback latency drops from seconds to tens of
milliseconds because the callback returns as soon as the *first* chunk is
durable. The remaining chunks drain in the background under back-pressure.

See `crates/pcloud-daemon/src/write_path.rs` and
`crates/pcloud-fs/benches/chunked_flush.rs`.

## C1 — `flush_latency_seconds` histogram

A Prometheus-compatible histogram was landed in `C1` and is observed from
**`WritePathService::chunked_flush`**, wrapping each chunk flush with a
`start_timer()` that records on drop (so cancellations and panics still
record).

### Bucket layout

```
buckets = [
    0.001, 0.005, 0.01, 0.025, 0.05,
    0.1,   0.25,  0.5,  1.0,   2.5,
    5.0,   10.0,
]  // seconds, plus the implicit +Inf bucket
labels  = ["outcome"]   // "ok" | "err" | "cancelled"
```

Twelve explicit buckets cover 1 ms – 10 s, which is the useful range for a
single chunk flush. Chunks that exceed 10 s fall into `+Inf` and trigger an
SLO alert.

### How to scrape

The daemon exposes `/metrics` on the local admin listener (opt-in, bound to
loopback). A minimal Prometheus scrape config:

```yaml
scrape_configs:
  - job_name: pcloudd
    static_configs:
      - targets: ['127.0.0.1:9131']
    metrics_path: /metrics
```

Sample query for p95 chunk-flush latency:

```promql
histogram_quantile(
  0.95,
  rate(flush_latency_seconds_bucket{outcome="ok"}[5m])
)
```

See `crates/pcloud-daemon/src/metrics.rs` for the registration site and
`operations/runbook.md` for alert thresholds.

## Reproducing the benches

```bash
cd .
cargo bench -p pcloud-fs --bench chunked_flush
cargo bench -p pcloud-fs --bench page_cache
cargo bench -p pcloud-embedded-sdk --bench upload_session
# JSON summaries land under target/criterion/
```

The `development/release-checklist.md` file references these benches as a
manual release gate until a benchmark workflow exists: a regression > 10 %
on any of `chunked_flush`, `upload_session`, or `page_cache` blocks the
release.

## If you're new to performance work on this codebase

The **thing to know**: every performance decision in the daemon is driven by
*tail latency*, not throughput. pCloud IO is TLS-bound and
`binapi.pcloud.com`-bound; raw throughput is whatever the server gives us.
What we optimise locally is the probability that *this* user interaction
spends milliseconds in kernel/CPU rather than tens-of-milliseconds. That is
why every "win" below is framed in terms of *worst-case* behaviour (a cache
eviction under a full LRU, a flush of a 128 MiB backlog, a TLS body of
10 GiB) rather than steady-state throughput.

If you are thinking about a performance change, answer these four questions
first:

1. **Does this change the tail?** Steady-state gains are nice; p99/p999
   wins are load-bearing.
2. **Does this change allocation volume on the hot path?** Every
   `Vec::push` in the read path has been examined; a new one should be
   justified.
3. **Does this change contention behaviour?** `parking_lot::Mutex` hold
   times should stay short; long-hold operations must move into a
   `RwLock` or a `tokio::sync::oneshot` hand-off.
4. **Is there a bench?** If not, the change has to bring one.

## Hotpaths

The daemon has three hotpaths under production load:

1. **Page-cache read** (`pcloud-fs::page_cache::get`). Hit path is
   `Arc::clone + HashMap::get`; miss path falls through to the network.
2. **Chunked write-flush** (`pcloud-daemon::write_path::chunked_flush`).
   Worst case walks the journal one chunk at a time with a bounded
   in-flight semaphore.
3. **Download stream window** (`pcloud-proto::transfer_api::download_stream`).
   TLS read → 64 KiB buffer → SHA256 feed → `File::write_all`, in a loop.

Everything else is either one-shot (auth, public-link create) or
rate-limited (engine walkers).

## Arc cache-hit semantics (deep dive)

The `Arc<Vec<u8>>` decision is the second-largest win after the O(1) LRU
swap. Its correctness is non-obvious enough to be worth its own section.

**Invariant**: once an `Arc<Vec<u8>>` is inserted into the cache, the buffer
it points to is **immutable** for the lifetime of that `Arc`. Writes do not
mutate in place; they invalidate and replace.

**Consequence**: any number of reader threads can hold their own
`Arc::clone` without any synchronisation beyond the atomic refcount. A reader
that started before an invalidation keeps observing the old page until it
drops its `Arc`; a reader that started after observes the new one. There is
no torn page, no read-while-write.

**Edge case** — a writer flushing a partially-updated page must produce a
*new* `Arc<Vec<u8>>` by cloning the old page, applying the diff, and
swapping it into the `HashMap`. This is the only place we pay the memcpy
cost; it is paid once per write-then-read cycle, not per read.

**Non-goal**: we do not share pages across file handles. An `Arc` page is
scoped to a `(file_id, offset)` key, and different files with identical
contents keep separate pages. Deduplication happens at the protocol layer,
not in the cache.

## Chunked flush (deep dive)

```
+------------------+    +--------------+    +--------------+
| journal head     |--->| pick chunk   |--->| upload_write |
| (durable offset) |    | (1 MiB slice)|    | (proto call) |
+------------------+    +--------------+    +--------------+
        ^                                           |
        |                                           v
        +---- advance head (fsync) <--- ack --------+
```

- Chunk size: `WRITE_FLUSH_CHUNK = 1 MiB`. Chosen to match the TLS record /
  reassembly sweet spot on real links (see
  [ADR-0008](../adr/0008.md) for the sibling download-side analysis).
- In-flight limit: `max_in_flight_chunks = 4`. A larger window buys nothing
  because the ack cycle is TLS-bound anyway; a smaller window starves the
  pipeline under burst load.
- Back-pressure primitive: `parking_lot::Condvar` + counter. Not a tokio
  semaphore, because the flush path is synchronous and a tokio dependency
  would drag an executor into the daemon hot path.
- Journal-head advance: a single `pwrite` into a sidecar file followed by
  `fsync(file) + fsync(dir)`. A crash before this `fsync` resumes from the
  last durable head.

A flush cancels cleanly at any chunk boundary. Chunks already in flight
finish and `drop()` their `start_timer`, so every partial flush still
contributes to `flush_latency_seconds` — cancellation appears as
`outcome="cancelled"` in the histogram.

## `flush_latency_seconds` histogram (deep dive)

The histogram is exposed at `/metrics` on the loopback admin listener. The
bucket layout is chosen explicitly so the interesting parts of the
distribution are resolved:

- `0.001 – 0.005 s`: a fully-cached chunk (rare; only on re-flush of an
  already-acked chunk after a journal restart).
- `0.005 – 0.1 s`: the typical hot chunk. Healthy networks live here.
- `0.1 – 1.0 s`: the p50..p99 for most real workloads.
- `1.0 – 10 s`: the unhappy tail. Alerts are configured here.
- `> 10 s` (`+Inf`): pathological. Pages an operator.

The `outcome` label is three-valued: `ok`, `err`, `cancelled`. `err`
records a flush chunk that returned a protocol error; the chunk is retried
under the resilience policy and may appear multiple times.

### Useful Prometheus recording rules

```
# p50 / p95 / p99 by outcome over 5m
pcloud:flush_latency:p50 = histogram_quantile(0.5,
  rate(flush_latency_seconds_bucket[5m]))
pcloud:flush_latency:p95 = histogram_quantile(0.95,
  rate(flush_latency_seconds_bucket[5m]))
pcloud:flush_latency:p99 = histogram_quantile(0.99,
  rate(flush_latency_seconds_bucket[5m]))

# error rate
pcloud:flush_error_rate = sum(rate(
  flush_latency_seconds_count{outcome="err"}[5m])
) / sum(rate(flush_latency_seconds_count[5m]))
```

## Tradeoffs and design decisions

- **Why not a lock-free LRU?** A lock-free doubly-linked list is hard to
  get right under concurrent mutation, and our hot-path profiler showed the
  `parking_lot::Mutex` held for the promotion step is ~40 ns. We revisit if
  the lock shows up in future flamegraphs.
- **Why `Arc<Vec<u8>>` instead of `bytes::Bytes`?** `bytes::Bytes` adds a
  ref-counted slice that we do not need; our page granularity is fixed and
  we never split a page. Direct `Arc<Vec<u8>>` is one fewer dependency and
  the same atomic-refcount cost.
- **Why blocking `ureq`/`rustls`?** Keeping the proto client synchronous
  lets us use `catch_unwind` around every protocol call without `.await`
  cancellation concerns. The request volume does not justify an executor.
- **Why 64 KiB download windows?** Measured: 16 KiB and 32 KiB showed
  higher syscall overhead; 128 KiB and above showed no additional gain on
  a 1 Gbit link and higher peak RSS. See [ADR-0008](../adr/0008.md).

## Concurrency model (performance-relevant)

- Hot-path locks are `parking_lot::Mutex`, held < 100 ns in the common
  case.
- The page cache's `HashMap<CacheKey, EntryHandle>` is guarded by a
  single `parking_lot::RwLock`; reads are the common case, writes are
  batched at page-eviction or invalidation time.
- Chunked-flush back-pressure uses a `Condvar`+counter, not a semaphore
  crate, because the counter is trivially inspectable in debuggers and in
  metrics.
- No `Arc<Mutex<…>>` cycles: we explicitly audit for those in review.

## Security invariants (performance-adjacent)

- The 1 MiB IPC body cap (`protocol.rs:20`) bounds the worst-case
  allocation a malformed client can force on the daemon.
- The bounded in-flight chunk count (`max_in_flight_chunks = 4`) bounds
  memory used by a runaway flush.
- The panic guard around `chunked_flush` records partial latency and
  increments a panic counter; a panic cannot hide behind "slow but
  alive".

## Extension points

- New bench: add a Criterion benchmark under the owning crate's `benches/`
  directory and wire it into `development/release-checklist.md` as a release
  gate if the path is hot.
- New metric: register in `crates/pcloud-daemon/src/metrics.rs` and
  document the bucket rationale here.
- Alternative cache backend: the page cache is fronted by a
  `PageCacheBackend` trait; a persistent-disk backend is a valid
  experiment, but the `Arc<Vec<u8>>` hit-path invariant must be
  preserved.

## Open `bd` trackers

- **`bd-1du`** — parity epic.
- **`bd-1du.4`** — FUSE/write-path work; the chunked-flush wiring lives in
  the same wave and is tracked here.
- **`bd-1du.4.6.1`** — write-path daemon wiring follow-ups (see
  [ADR-0010](../adr/0010.md)).
- **`bd-1du.10`** — final proof; release gating cites the benches in this
  page.

## Cross-references

- [Overview](./overview.md) — where performance sits in the larger
  architecture.
- [Crate Map](./crate-map.md) — `pcloud-fs`, `pcloud-daemon`, and the
  crate-local benchmark ownership.
- [Platform Support](./platform-support.md) — per-platform perf
  caveats (Windows flush semantics, macOS FUSE-t overheads).
- [Operations → Runbook](../operations/runbook.md) — alert thresholds
  for `flush_latency_seconds`.
- [ADR-0008](../adr/0008.md) — 64 KiB download buffer rationale.

## Canonical SLOs

The daemon exposes a canonical set of **Service-Level Objectives** via
the `/slo` HTTP endpoint (JSON) and the `Method::GetSlo` IPC surface
(`pcloudc slo`, `pcloudc --json slo`). The registry lives in
`crates/pcloud-observability/src/slo.rs` and backs every SLO with a
matching histogram/counter family already emitted on `/metrics`.

| SLO name                                   | Target         | Direction  | Window       |
|--------------------------------------------|----------------|------------|--------------|
| `ipc.request.latency.p99`                  | `< 100 ms`     | upper      | rolling 5 m  |
| `ipc.request.error_rate`                   | `< 0.1 %`      | upper      | rolling 5 m  |
| `auth.login.success_rate`                  | `> 99 %`       | lower      | rolling 1 h  |
| `upload.throughput_mbps.p50`               | `> 5 MB/s`     | lower      | rolling 5 m  |
| `mount.read.latency.p99`                   | `< 50 ms`      | upper      | rolling 5 m  |
| `integrity_sweeper.run.p95`                | `< 5 min`      | upper      | per-run      |
| `audit.hash_chain.verify.daily_pass_rate`  | `> 99.9 %`     | lower      | daily        |

### Honesty

These thresholds are **aspirational targets** against which the live
metrics are compared. Real measured values come straight from the
daemon's atomic counters — the current pre-GA build **does not**
uniformly meet every SLO under load, and the registry does not fudge
that. SLOs without enough samples report `status: "no_data"` so
dashboards never conflate "quiet" with "healthy".

### Wire shape

`GET /slo` (HTTP) and `pcloudc slo` (IPC) both return a JSON document
with a `slos` array whose entries are shaped:

```json
{
  "slo_name": "ipc.request.latency.p99",
  "target":   "<100ms",
  "actual":   "42.5ms",
  "status":   "ok"
}
```

`status` is one of `ok` / `violation` / `no_data`. The aggregate `pass`
bit in the enclosing document is `true` when no entry is in the
`violation` state.

### Instrumentation call sites

Each SLO is fed by an existing metric family; the SLO registry is a
separate, lock-free view sitting alongside the Prometheus exposition:

- `ipc.request.latency.p99` ← IPC dispatch latency (`Slo::observe_ipc_latency`)
- `ipc.request.error_rate` ← dispatch outcome classifier
  (`Slo::observe_ipc_outcome`)
- `auth.login.success_rate` ← auth backend
  (`Slo::observe_auth_login`)
- `upload.throughput_mbps.p50` ← transfer backend, per completed upload
  chunk (`Slo::observe_upload_throughput_mbps`)
- `mount.read.latency.p99` ← FUSE read path
  (`Slo::observe_mount_read_latency`)
- `integrity_sweeper.run.p95` ← per-cycle timing inside
  `integrity_sweeper_service` (`Slo::observe_integrity_sweeper_run`)
- `audit.hash_chain.verify.daily_pass_rate` ← daily `audit verify`
  cron / on-demand run (`Slo::observe_audit_verify`)

All 7 canonical SLO call sites are wired as of 2026-04-16:

1. **IPC latency + error rate** — `RuntimeShell::handle_request` in
   `crates/pcloud-daemon/src/runtime.rs` (unconditional, every dispatch).
2. **Auth login success rate** — `auth_response` in `runtime.rs`,
   triggered on `AuthEvent::LoginSucceeded` / `LoginFailed`.
3. **Upload throughput** — upload completion path in `runtime.rs`,
   after each `upload_bytes` round-trip.
4. **Mount read latency** — `BoxedFuserShim::read` and
   `PcloudFsShim::read` in `crates/pcloud-fs/src/platform/linux.rs`,
   via the process-wide `slo_hook::observe_mount_read`.
5. **Integrity sweeper run** — scheduler loop in
   `integrity_sweeper_service.rs`, via `slo_hook::observe_integrity_sweeper_run`.
6. **Audit chain verify** — `audit_verify_chain` in `runtime.rs` (on-demand)
   and `AuditVerifierShell::run_once` / `scheduler_loop` in
   `audit_verifier_service.rs` (scheduled).

SLOs without enough samples still report `status: "no_data"` so
dashboards never conflate "quiet" with "healthy". Adding a new
observation site does **not** require any change to the `/slo` document
shape — the registry rolls forward silently.

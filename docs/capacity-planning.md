# Capacity Planning

## 1. Overview

This document gives operators a starting point for sizing a `pcloud-rs`
deployment. It is a **planning aid**, not a production-validated SLA
sheet.

Most numbers in this document are tagged `[ESTIMATE]`. They are
extrapolations from the in-tree config defaults and from code review,
not from a measured baseline. The empirical baseline that will replace
these estimates is tracked under **plan task T3.6** (heaptrack memory
profiling baseline) in `CLAUDEREV/TIER-PROGRESS.md`.

### Relationship to T3.6

T3.6 is currently `[OUT-OF-SCOPE-PENDING-USER-RESOURCE]` because it
requires a 24-hour live sync run with `heaptrack` attached, plus a CI
job that records the baseline. Until T3.6 lands, the numbers below are
order-of-magnitude estimates so operators can plan hardware
provisioning without waiting on the baseline.

When T3.6 produces measured numbers, this document MUST be updated:
each `[ESTIMATE]` row should be replaced (or kept and annotated as
`[MEASURED]` with date + heaptrack commit). The doc structure is
designed so that swap is a row-level edit, not a rewrite.

For operational procedures referenced below (mount recovery, log
rotation, vault backup), see [`OPERATIONS-RUNBOOK.md`](../OPERATIONS-RUNBOOK.md).

---

## 2. Per-Resource Estimates

These per-resource numbers are the building blocks the sizing table in
section 3 multiplies out.

| Resource | Value | Source |
|---|---|---|
| FUSE page size | **64 KiB** | Config default: `pcloud_cache::page_cache_generic::DEFAULT_PAGE_SIZE = 64 * 1024` (`crates/pcloud-cache/src/page_cache_generic.rs:43`). |
| Page-cache memory budget per mount | **256 MiB** | Config default: `mount.cache_size_mb = 256` (`crates/pcloud-config/src/mount.rs:71`). |
| Metadata-cache entries per mount | **4096** | Config default: `mount.page_cache_entries = 4096` (`crates/pcloud-config/src/mount.rs:75`). |
| Full-scan interval per sync root | **300 s** (5 min) | Config default: `sync.full_scan_interval_secs = 300` (`crates/pcloud-config/src/sync_loop.rs:39`). |
| RAM per active sync root (engine state + shared LRU) | **`[ESTIMATE]` ~50 MB** | Engine planner + journal + shared page cache (the 256 MiB cache is shared across mounts, not multiplied per root). |
| Disk per cached page (64 KiB on-disk) | **`[ESTIMATE]` ~70 KiB** | 64 KiB payload + index/journal/checksum overhead. |
| Network per active mount (idle) | **`[ESTIMATE]` 5–50 KB/s** | Heartbeat + diff polls every `full_scan_interval_secs` (300 s default) plus mount keep-alive. |
| Process RSS baseline (idle, 0 sync roots) | **`[ESTIMATE]` 80–120 MB** | Tokio runtime + SQLite handle + IPC listener + crypto/TLS state. |
| Process RSS additional cost per sync root | **`[ESTIMATE]` ~15 MB** | Engine planner + per-root SQLite cursors + journal buffers. |

> Note: the page-cache budget (`mount.cache_size_mb`, default 256 MiB)
> is a **per-mount** budget, not a per-sync-root budget. Multiple sync
> roots that share a single FUSE mount share the cache. A deployment
> with one mount and 5 sync roots is not 5 × 256 MiB of cache.

---

## 3. Sizing Table by Deployment Scale

| Scale | Sync roots | Disk cache | Process RSS | Network (idle) | Notes |
|---|---|---|---|---|---|
| **Single-user laptop** | 1–3 | 100 MB – 1 GB | `[ESTIMATE]` ~150 MB | `[ESTIMATE]` 5–150 KB/s | Default `cache_size_mb = 256`; default `page_cache_entries = 4096`. Suitable for a single primary mount + a couple of backup roots. |
| **NAS / always-on host** | 5–10 | 1 GB – 10 GB | `[ESTIMATE]` ~500 MB | `[ESTIMATE]` 25–500 KB/s | Raise `cache_size_mb` to `2048`–`8192` and `page_cache_entries` to `16384`–`65536`. Stagger `full_scan_interval_secs` per root to avoid scan storms. |
| **Fleet (per-host)** | Same as laptop, × N hosts | Same as laptop, × N hosts | Same as laptop, × N hosts | Same as laptop, × N hosts | Sizing is per-host. Multiply by fleet size for centrally-billed bandwidth. Use the bandwidth scheduler (`[bandwidth.schedule]`) to throttle aggregate egress. |

All RSS / network numbers in this table inherit the `[ESTIMATE]` tag
from section 2.

---

## 4. Recommended Config Knobs by Scale

The knobs below are the supported tuning surface. Authoritative
reference: `docs/book/src/reference/config.md`.

### Per-mount memory and cache

- **`mount.cache_size_mb`** — page-cache memory budget in MiB.
  Default `256`. Env override: `PCLOUD_MOUNT_CACHE_SIZE_MB`. The env
  var `PCLOUD_CACHE_SIZE_GB` takes precedence over both.
  - Laptop: keep default `256`.
  - NAS: raise to `2048`–`8192`.
  - Fleet: pick the laptop default unless per-host workload is heavy.
- **`mount.page_cache_entries`** — number of metadata-cache entries
  (LRU). Default `4096`. Env override:
  `PCLOUD_MOUNT_PAGE_CACHE_ENTRIES`.
  - Laptop: keep default `4096`.
  - NAS with many small files: raise to `16384`–`65536`.

### Per-sync-root scheduling

- **`sync.full_scan_interval_secs`** — full-scan cadence. Default
  `300` (5 min). Bounds enforced in
  `crates/pcloud-config/src/sync_loop.rs`: must be in `[30, 86400]`.
  - Laptop: keep default `300`.
  - NAS with many roots: raise to `900`–`3600` to spread scan load.

### Bandwidth shaping

- **`[bandwidth.schedule]`** — time-of-day rules and metered-network
  cap. See `crates/pcloud-config/src/bandwidth_schedule.rs` for the
  schema. Use this for fleet egress control rather than scaling
  `cache_size_mb` down.

---

## 5. How to Validate (Replacing `[ESTIMATE]` Numbers)

The following procedure ties this document to T3.6. It is the
canonical path for replacing each `[ESTIMATE]` row above with a
`[MEASURED]` row.

### Prerequisites

- Linux host with `heaptrack` installed (`apt install heaptrack` or
  distro equivalent).
- A live pCloud account configured per the
  [`OPERATIONS-RUNBOOK.md` "Live E2E account setup"](../OPERATIONS-RUNBOOK.md#live-e2e-account-setup) playbook.
- `pcloudd` built in release mode: `cargo build --release -p pcloud-daemon`.

### Procedure

1. **Establish the baseline (idle, 0 sync roots).**

    ```sh
    heaptrack --output /tmp/pcloudd-idle.heaptrack \
        ./target/release/pcloudd --foreground
    # let it run for 10 minutes, then SIGTERM.
    heaptrack_print /tmp/pcloudd-idle.heaptrack.zst | tail -50
    ```

    Record peak RSS and peak heap. Update **"Process RSS baseline"** in
    section 2 with the measured value, replace `[ESTIMATE]` with
    `[MEASURED YYYY-MM-DD]`.

2. **Per-sync-root delta.** Add one sync root at a time (1, 2, 3, 5,
   10) and re-record peak RSS at each step. The slope is the
   per-sync-root cost. Update **"Process RSS additional cost per sync
   root"**.

3. **24-hour cache-warmup run.** With 3 sync roots and active read+write
   traffic, leave `pcloudd` running for 24 hours under heaptrack. This
   is the actual T3.6 acceptance run. Update the per-resource RAM /
   network rows in section 2.

4. **Update the sizing table.** Recompute section 3 by multiplying the
   measured per-resource numbers by the deployment scale.

5. **Cross-check with Prometheus.** While the run is live, scrape the
   metrics endpoint and confirm `pcloud_cache_*` / `pcloud_engine_*`
   counters match expectations. See
   [`OPERATIONS-RUNBOOK.md` "Health checks"](../OPERATIONS-RUNBOOK.md#health-checks).

### Acceptance for replacing this doc

A row may be promoted from `[ESTIMATE]` to `[MEASURED YYYY-MM-DD]`
when:

- the heaptrack output is committed under `docs/baselines/` (or
  archived per fleet retention policy),
- the measurement was taken on a release-mode build at a recorded git
  commit,
- at least one operator has reproduced the number on a second host.

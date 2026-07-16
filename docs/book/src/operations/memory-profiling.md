# Memory Profiling (heaptrack)

This page documents the memory-profiling harness that backs
[`CLAUDEREV/TIER-PROGRESS.md`](../../../../CLAUDEREV/TIER-PROGRESS.md)
row **T3.6 — Memory profiling**. Acceptance is a published peak-RSS
baseline plus a per-PR alert if RSS regresses by 10% or more.

## What runs where

The harness has three pieces:

- **`tools/memprofile/run.sh`** — Bash driver. Builds `pcloudd` in
  release mode, spins up a hermetic dev-mode profile under a tempdir
  (no network, no live account), runs `heaptrack pcloudd` for
  `RUN_DURATION_SECS` seconds while synthesising sync activity (touch /
  list / delete files in a fake sync root), then post-processes the
  capture with `heaptrack_print --json` and compares peak RSS against
  the baseline.
- **`tools/memprofile/baseline.json`** — JSON baseline (schema
  `pcloud-rs/memprofile/baseline-v1`). Records `peak_rss_bytes`,
  `total_allocations`, `run_duration_secs`, and `recorded_at`. The file
  is checked into the repo so every PR gates against the same number.
- **`.github/workflows-disabled/memprofile.yml`** — archived reference for
  the former scheduled job. GitHub Actions is inactive; operators invoke
  `tools/memprofile/run.sh` locally and retain the output directory.

## Platform support

heaptrack is a Linux-only tool. The driver script refuses to run on non-Linux hosts
(exit code 2). macOS and Windows have no equivalent in this fork — if
you need a memory profile on those platforms, use Instruments or the
WPA / Heap Profiler, capture the same `peak_rss_bytes` shape, and feed
the resulting `baseline.json` into the same comparison logic by hand.

## How the gate works

The script computes a regression threshold as

```text
threshold = baseline.peak_rss_bytes * (100 + REGRESSION_THRESHOLD_PCT) / 100
```

with `REGRESSION_THRESHOLD_PCT = 10` by default. If the current run's
peak RSS exceeds the threshold, the job exits 1 (FAIL). Otherwise it
exits 0 (PASS).

Exit codes:

| Code | Meaning                                                                     |
|------|-----------------------------------------------------------------------------|
| 0    | PASS, OR baseline was just initialised (cold start), OR `--update-baseline` |
| 1    | FAIL — peak RSS regressed by >= 10% versus baseline                         |
| 2    | USAGE — bad arg, missing tool, non-Linux host, or hermetic setup failure    |
| 3    | RUNTIME — heaptrack itself failed, or JSON metric extraction failed         |

## Run durations

`RUN_DURATION_SECS` controls how long heaptrack stays alive:

- **60 s (default for local invocation):** enough to prove the harness
  works end-to-end. Insufficient for a meaningful baseline.
- **900 s = 15 min (weekly CI cron):** the workflow's default. Catches
  regressions in steady-state allocation patterns.
- **86400 s = 24 h (operator-driven soak):** the T3.6 plan acceptance
  criterion. Trigger via `workflow_dispatch` with `run_duration_secs:
  86400`. Note: GitHub-hosted runners cap at 360 minutes; full 24-hour
  soaks must run on a self-hosted long-running runner.

## Bumping the baseline

When a PR intentionally increases memory use (e.g. a new feature that
adds a real cache), the operator runs:

```bash
tools/memprofile/run.sh --update-baseline
```

on a Linux host with `heaptrack` and `jq` installed. The flag is
**operator-only**: the CI workflow does NOT pass it during cron runs,
so the baseline can never be silently rewritten by an automated job.

The workflow exposes an `update_baseline` boolean input on
`workflow_dispatch`; when set to `true` and the workflow runs on the
`development` branch, the job produces a `git diff` of `baseline.json`
in its log output. The operator copies that diff into a follow-up PR
manually — the workflow does **not** push directly.

## Interpreting heaptrack output

`heaptrack_print --json` produces a JSON document with at least these
keys (schema may vary by heaptrack version):

- `peakRSS` — peak resident set size in bytes during the run.
- `totalAllocations` — total number of allocations across the run.
- `peakHeapMemory`, `peakLeakedSize`, etc. — see `heaptrack_print
  --help`.

The script tolerates both top-level (`.peakRSS`) and nested
(`.summary.peakRSS`, `.totals.peakRSS`) shapes. If you upgrade
heaptrack and the JSON layout changes, update the `jq` paths in
`tools/memprofile/run.sh` accordingly.

For interactive analysis of a captured run, use the GUI:

```bash
heaptrack_gui memprofile-out/pcloudd.heaptrack.zst
```

The GUI shows flame graphs, allocation hot spots, and leak suspects.
Use it to root-cause a regression before deciding whether to fix the
code or bump the baseline.

## Known limitations

- The hermetic profile runs `pcloudd` in offline dev mode, so the
  profile measures the daemon's resident allocations only, not
  pCloud-server interactions. End-to-end soak with a real account must
  run against a live test box (out of CI scope; tracked in
  `CLAUDEREV/TIER-PROGRESS.md` row T3.6 follow-ups).
- The synthesised sync activity (touch / list / delete every 5 s) is a
  smoke load, not a workload model. A representative production
  workload for the 24-hour soak is the operator's responsibility.
- `heaptrack` adds ~5–15% allocation overhead. Treat the recorded
  `peak_rss_bytes` as an upper-bound proxy, not the un-instrumented
  steady-state RSS.

## Cross-references

- Capacity planning ties RSS targets to recommended deployment shapes:
  `docs/capacity-planning.md`.
- The cold-start latency bench (T3.7) is the sibling regression-gate
  harness: `crates/pcloud-daemon/benches/cold_start.rs`.
- Operations runbook: `OPERATIONS-RUNBOOK.md`.

# Disaster Recovery Drill Harness

Scripted DR drills that exercise documented runbook procedures from
`OPERATIONS-RUNBOOK.md`. The harness is invoked monthly by
`.github/workflows/dr-drill.yml` (cron `0 0 1 * *`).

## Layout

- `run.sh` — top-level driver. Runs every script in `scenarios/` and
  aggregates exit codes. Exits non-zero if any scenario reports
  `[FAIL]`.
- `scenarios/<name>.sh` — one drill per file, `set -euo pipefail`,
  bash. Each must emit exactly one of:
    - `[PASS] <scenario>` on success,
    - `[FAIL] <scenario>: <reason>` on failure (exit 1),
    - `[SKIP] <scenario>: <reason>` when a documented runbook
      procedure does not yet exist (exit 77).

## Exit codes

- `0`  — all scenarios PASS or SKIP.
- `1`  — at least one FAIL. Blocks the next release tag per
  `CLAUDEREV/TIER-PROGRESS.md` row T4.2.
- `77` — used by individual scenarios to indicate a runbook gap;
  surfaced as SKIP by `run.sh` and does **not** fail the drill, but
  IS counted in the summary.

## Adding a scenario

1. Drop a new `scenarios/<name>.sh` (chmod +x).
2. Call `dr_setup` / `dr_teardown` from `scenarios/_common.sh`.
3. Emit a single `[PASS|FAIL|SKIP]` line as the last action.
4. Reference the OPERATIONS-RUNBOOK.md anchor the drill validates.

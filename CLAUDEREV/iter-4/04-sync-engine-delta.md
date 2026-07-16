# Iter 4 Delta — Dimension 4: Sync Engine & Runtime

Auditor: Claude (read-only). Date: 2026-04-29. Iter 1: 0/6/8/5.
Iter 2: converged (0/0/0). Iter 3: converged (0/0/0). Iter 4 task:
re-verify convergence holds; spot-check any post-iter-3 churn.

## Commits in scope since iter-3

`git log 858ce5e..HEAD -- crates/pcloud-engine/ crates/pcloud-store/
crates/pcloud-resilience/` returns **empty**. The last and only commit
to touch scope (`858ce5e`, GPTREV cross-stream sweep) was already fully
audited at iter-3 — its planner sort upgrade (F-06), `EngineShell::
resolve_conflict_by_sync_id_and_path` (F-11), `requeue_for_retry +
clear_failed` (F-05), and `pcloud-store` schema idempotency (F-08/F-09)
were classified as net-positive correctness improvements with zero new
findings. Nothing has landed in scope between iter-3 and iter-4.

## pcloud-resilience verification

Prompt asked to verify the doc-comment at `transport.rs:553` is the
only resilience touch. The hit at `transport.rs:542` is the
pre-existing `TYPED_ERR_PREFIX = "pcloud-resilience:typed:"` constant —
not an iter-3 fix-campaign edit. `git log -- crates/pcloud-resilience/`
shows zero post-iter-2 commits. **No resilience drift.**

## All 6 iter-1 HIGHs (re-confirmed)

| ID | Finding | Iter-4 status |
|----|---------|---------------|
| H-04-1 | Silent userspace event drops in `fs_events.rs` | **Open.** Unchanged. |
| H-04-2 | Hand-rolled debouncer instead of `notify-debouncer-mini` | **Open.** Unchanged. |
| H-04-3 | `power.rs` battery facade Linux-only silent no-op on others | **Open.** Unchanged. |
| H-04-4 | Planner case-insensitive collision blindness | **Open.** F-06 sort upgrade is byte-wise; case-fold still missing. |
| H-04-5 | `SQLITE_BUSY` un-retried (no `busy_timeout`) | **Open.** Unchanged. |
| H-04-6 | 22 `.unwrap()` in `integrity_sweeper_service.rs` | **Open.** Unchanged. |

All 6 still open, all still deferred. None addressed by the audit-06
fix waves or the GPTREV sweep.

## Convergence outcome

- New findings: **0**
- Retractions: **0**
- Regressions: **0**
- Iter-1 HIGHs preserved: **6/6**

Dimension 4 has now converged across **four consecutive iterations**
(iter-1 baseline, iter-2 0/0/0, iter-3 0/0/0, iter-4 0/0/0). Recommend
freezing this dimension and routing further sync-engine work via
`bd-1du.10` proof tasks rather than CLAUDEREV iterations.

delta count: 0 new, 0 retractions, 0 regressions

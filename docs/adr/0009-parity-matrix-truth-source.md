# ADR 0009: `STATUS.md` Is the Parity Truth Source

- Status: Accepted
- Date: 2026-04-15

## Context

Over the lifetime of the rewrite the project has accumulated several
places where "how much parity have we achieved" is stated:

- `README.md` — user-facing, prone to aspirational wording.
- `CLAUDE.md` — agent handoff doc, updated frequently by different
  agents.
- `STATUS.md` — dedicated status file at the workspace root.
- `C_FEATURE_PARITY_MATRIX.csv` — row-level machine-readable
  matrix.
- `C_FEATURE_PARITY_REVIEW.md` — narrative review.

When these drift, the consequences have been real: release wording
claiming "parity" while the matrix still shows `Partial` rows; handoff
docs stating counts that no longer match the CSV; PR descriptions
citing stale numbers.

The problem is not that there are multiple documents — each has a
legitimate audience — but that there is no declared precedence, so
every reader invents their own.

## Decision

`STATUS.md` is the **single authoritative source** for
Rust-side parity status. Specifically:

1. Overall counts (Implemented / Partial / Missing / Rejected),
   open-bead list, and the short "is it ready to ship" verdict live in
   `STATUS.md`. Everything else that mentions these numbers must cite
   `STATUS.md` or defer to it.
2. `C_FEATURE_PARITY_MATRIX.csv` is the row-level source of truth for
   individual feature rows; `STATUS.md` aggregates it.
3. `CLAUDE.md`, `README.md`, `C_FEATURE_PARITY_REVIEW.md`, release
   notes, and PR descriptions are **consumers**. They must reflect
   `STATUS.md`; on drift, `STATUS.md` wins.
4. A PR that changes aggregate parity claims must update `STATUS.md`
   in the same PR. CI enforces that a CSV row count change and a
   `STATUS.md` edit co-occur.

## Consequences

Good:

- Reviewers have a single file to check when a PR description and
  `CLAUDE.md` disagree. No ambiguity about which is right.
- Release wording has a mechanical pre-flight check: "does
  `STATUS.md` currently say what you are about to claim publicly?"
- Agents doing capability audits have an obvious place to post the
  result; no more racing edits to three files.
- Enables a simple CI guard (`STATUS.md` hash + row-count check) that
  catches matrix edits landing without their narrative counterpart.

Bad:

- Contributors must remember to touch `STATUS.md` when they change
  the matrix. Mitigated by the CI guard and by PR-template
  reminders.
- `README.md` and `CLAUDE.md` now have a normative upstream. Edits to
  them that contradict `STATUS.md` are defects, not improvements,
  even if the wording reads better in isolation.

Practical rule of thumb for any contributor (human or agent):

- Changing a single row's status in the CSV → update `STATUS.md`
  aggregate and any affected narrative in the same PR.
- Changing wording in `README.md` or `CLAUDE.md` about parity →
  first read `STATUS.md`; if the new wording contradicts it,
  the bug is in the wording, not in `STATUS.md`.

## Alternatives Considered

- **Make the CSV the single source**: rejected — CSV is row-level and
  not well-suited to "verdict" text that humans need. The
  aggregation layer adds real value.
- **Make `CLAUDE.md` the source**: rejected — `CLAUDE.md` is an
  agent-handoff dossier and changes shape with workflow, not just
  with parity state.
- **Generate `STATUS.md` automatically from the CSV**: considered;
  partial generation (counts) is feasible and will likely land in a
  future ADR. The narrative sections (open blockers, verdict)
  remain hand-written, so full auto-generation is deferred.

# ADR 0001: Architecture Decision Record Format

- Status: Accepted
- Date: 2026-04-15

## Context

The Rust rewrite of `pcloud-rs` spans several crates, a daemon, an SDK, and
multiple security-sensitive subsystems (auth, crypto, IPC, filesystem). Over
the course of the P0/P1/P2 hardening phases we have accumulated a number of
non-obvious decisions — mutex choice, panic handling, token persistence
layout, transport framing — that are easy to forget and expensive to
re-litigate.

Without a written record, future contributors (and future agents) have no
durable way to tell "deliberate choice" from "accidental leftover". That
ambiguity has already cost review cycles during the P0–P2 phases.

## Decision

All non-trivial architecture and security decisions in `` are
recorded as Architecture Decision Records (ADRs) under `docs/adr/`, one
Markdown file per decision, numbered sequentially
(`NNNN-short-slug.md`, zero-padded to four digits).

Every ADR uses the following sections, in this order:

1. **Title line** — `# ADR NNNN: <Short Title>`
2. **Metadata** — bulleted `Status` and `Date` lines directly under the title.
3. **Context** — what prompted the decision; constraints; prior art.
4. **Decision** — the chosen direction, stated in the present tense.
5. **Consequences** — what follows from the decision, both good and bad,
   including operational and security impact.
6. **Alternatives Considered** — other options evaluated and why they were
   rejected. "None" is an acceptable answer only when it is actually true.

Status values are constrained to:

- `Proposed` — written up but not yet ratified; implementation may exist but
  reviewers have flagged ambiguity.
- `Accepted` — ratified and reflected in code.
- `Superseded by ADR NNNN` — kept for history; do not delete.
- `Deprecated` — no longer applies but historical rationale retained.

Date uses ISO 8601 (`YYYY-MM-DD`) and is the date the ADR was last materially
edited, not the date of the underlying decision — a separate "originally
decided" line inside Context may be used when that distinction matters.

ADRs are append-only. If a decision changes, write a new ADR that
supersedes the old one and update the old ADR's status line. Never rewrite
history.

## Consequences

Good:

- Decisions have a single, linkable home; reviewers can cite
  `docs/adr/0004-...` instead of re-explaining rationale in every PR.
- Onboarding is faster: a new contributor can read `docs/adr/README.md` and
  get the shape of the system without trawling commit history.
- CLAUDE.md and STATUS.md stop being decision dumping-grounds; they can
  point at ADRs for the "why".

Bad:

- Modest documentation overhead per non-trivial change.
- Risk that ADRs go stale if contributors forget to write new ones when
  supersession happens. Mitigation: PR review expectation — any
  behaviour-changing PR that touches a subsystem with an ADR must either
  cite it, amend it, or add a superseding ADR.

## Alternatives Considered

- **Design docs in `docs/book/`**: considered, but long-form design docs
  are higher-ceremony and tend to describe aspirational state. ADRs
  describe decisions as of a point in time and coexist well with design
  docs.
- **Commit messages only**: insufficient — commits do not survive rebases
  cleanly, are not easily indexed, and mix implementation churn with
  rationale.
- **Issue tracker (`bd`) threads**: used for work planning, but issues
  close and rationale is lost. ADRs are the durable counterpart.

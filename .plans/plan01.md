# Plan 01

## Goal

Bring the repository back to an honest, auditable, production-gated state by fixing the gaps found in the audit across governance, security controls, platform claims, and beginner-facing documentation.

## Scope

- Release-truth and governance artifacts
- Tracker and blocker visibility
- CI and release evidence
- Runtime security/control-path mismatches
- Platform support truthfulness
- Config, path, and IPC documentation consistency
- Beginner onboarding and handbook integrity
- Objective release gates and verification

## Non-goals

- Shipping new product features unrelated to the audit findings
- Expanding platform support beyond what is required to make current claims true
- Cosmetic documentation rewrites that do not improve correctness or operator usability

## Principles

1. Code and committed artifacts are the authority.
2. Packaging presence does not imply runtime support.
3. Security controls must either exist on the active path or be documented as absent.
4. Beginner docs must optimize for correct first execution, not feature exhaustiveness.
5. Every release/readiness claim must map to a test, script, workflow, or artifact.

## Workstreams

### 1. Release Truth And Governance

- Reconcile README, STATUS, CLAUDE, CONTRIBUTING, and book language to one release posture.
- Restore or remove `bd`-based blocker claims.
- Rebuild an auditable readiness story that does not depend on missing state.

### 2. CI And Evidence Restoration

- Restore missing workflow definitions or remove references to them.
- Ensure release checklists only mention gates that are present and inspectable.
- Rebuild the audit trail for security and packaging evidence.

### 3. Runtime Security And Control Paths

- Resolve the policy-engine documentation/code mismatch.
- Fix unsafe or misleading mount/runtime behavior on BSD and other non-Linux targets.
- Ensure startup fails closed where the code already says unsafe states should be rejected.

### 4. Platform Truthfulness

- Reduce support claims to match actual verified runtime behavior.
- Separate compile/package support from functional runtime parity.
- Re-document unsupported or unverified mount targets conservatively.

### 5. Config, Paths, And IPC Consistency

- Define the canonical config model for CLI and daemon.
- Align all docs and help text around the actual path helpers and file formats.
- Eliminate primary-path confusion between XDG and legacy `~/.pcloud` fallback.

### 6. Beginner Documentation Repair

- Rebuild install, first-login, and first-sync guides around the real command surface.
- Remove dead links, stale examples, and broken references.
- Fill crate README gaps where the root docs promise crate-local orientation.

### 7. Release Gate Hardening

- Provide reproducible doc-validation instructions and tooling.
- Split readiness criteria into objective code, docs, packaging, and live-verification gates.
- Back strong claims with automated tests where possible.

## Execution Order

1. Release truth and governance
2. CI and evidence restoration
3. Runtime security and control paths
4. Platform truthfulness
5. Config, paths, and IPC consistency
6. Beginner documentation repair
7. Release gate hardening

## Milestones

### Milestone A: Honest Repository State

- No stale production or tracker claims remain.
- All blocker references resolve to real tracker items or are removed.
- Missing audit/workflow references are restored or deleted.

### Milestone B: Security And Runtime Truth

- Policy enforcement is either integrated and tested or explicitly documented as not active.
- BSD/non-Linux mount behavior is no longer misleading.
- Unsafe startup paths fail closed where required.

### Milestone C: Usable Documentation

- Beginner path is short, correct, and reproducible.
- Handbook links resolve.
- Config/path/IPC guidance is internally consistent.

### Milestone D: Defensible Release Gate

- Release checklist is evidence-based.
- Docs build path is reproducible.
- Claimed CI/release gates are present and inspectable.

## Exit Criteria

- The repository no longer over-claims production readiness.
- Security and enterprise docs match active runtime behavior.
- Platform support text matches tested behavior, not aspiration.
- A new contributor can follow install, login, and first-sync docs without path/config confusion.
- Another engineer can audit readiness from the repo alone.

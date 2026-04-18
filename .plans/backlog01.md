# Backlog 01

## Priority Legend

- `P0` release-truth or security blocker
- `P1` major documentation or platform correctness task
- `P2` important cleanup that improves auditability and onboarding

## Workstream 1: Release Truth And Governance

### `P0` Task 1.1: Unify the repo-wide release posture

- Files:
  - `README.md`
  - `STATUS.md`
  - `CLAUDE.md`
  - `CONTRIBUTING.md`
  - `docs/book/src/introduction.md`
  - `docs/book/src/parity/status.md`
- Actions:
  - Choose one supported wording for current status, likely `pre-alpha` or `not production-ready`.
  - Remove any conflicting wording about enterprise readiness, full parity, or drop-in replacement.
  - Ensure the same blocker language appears in all top-level truth docs.
- Acceptance:
  - All listed files describe the same release posture.
  - No file claims production readiness while blocker language remains.

### `P0` Task 1.2: Repair the tracker story

- Files:
  - `README.md`
  - `STATUS.md`
  - `CONTRIBUTING.md`
  - `CLAUDE.md`
  - any `docs/**` file naming `bd-1du*`
- Actions:
  - Decide whether `bd` remains the source of truth.
  - If yes, restore/import the actual issue database and recreate the blocker issues.
  - If no, remove all wording that tells contributors to trust `bd` for current blockers.
  - Add one explicit contributor section explaining how tracker state is initialized and where it lives.
- Acceptance:
  - `bd status` and documented blocker counts agree, or docs no longer claim live `bd` state.
  - No doc references open beads that do not exist.

### `P0` Task 1.3: Remove hard dependency on missing evidence files

- Files:
  - `README.md`
  - `SECURITY.md`
  - `CHANGELOG.md`
  - `docs/book/src/security/audit-dossier.md`
  - any other file referencing `SECURITY-AUDIT-FINAL-14042026.md`
- Actions:
  - Either add the missing audit file to the repo or replace the references with an existing artifact.
  - Verify every linked evidence artifact exists.
- Acceptance:
  - No top-level or book document links to a missing evidence file.

## Workstream 2: CI And Evidence Restoration

### `P0` Task 2.1: Restore or de-reference missing GitHub workflow files

- Files:
  - `.github/workflows/rust.yml`
  - `.github/workflows/packaging.yml`
  - `.github/workflows/release.yml`
  - docs referencing those files
- Actions:
  - If these workflows are intended to exist in-repo, add them with the gates the docs claim.
  - If workflow ownership is external, rewrite docs to point to the actual source of truth instead of nonexistent local paths.
- Acceptance:
  - Every workflow filename named in docs exists, or no doc mentions it.

### `P0` Task 2.2: Align release checklist with actual executable gates

- Files:
  - `docs/book/src/development/release-checklist.md`
  - `docs/book/src/reference/packaging.md`
  - `docs/book/src/operations/packaging-matrix.md`
  - `README.md`
  - `CONTRIBUTING.md`
- Actions:
  - Enumerate the real gates that can be run today.
  - Remove aspirational jobs and unverifiable steps.
  - For each remaining gate, include the exact command or workflow path.
- Acceptance:
  - Every checklist item maps to a real local command, workflow, or artifact.

### `P1` Task 2.3: Add a repo-local validation script for claimed gates

- Files:
  - `scripts/validate-release-readiness.sh` or similar
  - `README.md`
  - `CONTRIBUTING.md`
  - `docs/book/src/development/release-checklist.md`
- Actions:
  - Add one wrapper that runs the agreed local gates.
  - Include code, docs, and optional supply-chain checks.
- Acceptance:
  - One documented command exercises the repo’s local readiness gates.

## Workstream 3: Runtime Security And Control Paths

### `P0` Task 3.1: Decide the fate of policy enforcement

- Files:
  - `docs/enterprise/policy.md`
  - `docs/enterprise/README.md`
  - `crates/pcloud-daemon/Cargo.toml`
  - `crates/pcloud-daemon/src/bootstrap.rs`
  - `crates/pcloud-daemon/src/runtime.rs`
  - `crates/pcloud-daemon/src/dispatch.rs`
- Actions:
  - Choose one path:
    - integrate `pcloud-policy` into the daemon, or
    - explicitly mark policy as not wired into the active runtime.
  - Do not leave docs claiming deny-by-default enforcement if dispatch does not enforce it.
- Acceptance:
  - Runtime behavior and enterprise docs match.

### `P0` Task 3.2: Integrate policy engine if policy remains a shipped feature

- Files:
  - `crates/pcloud-daemon/Cargo.toml`
  - `crates/pcloud-daemon/src/bootstrap.rs`
  - `crates/pcloud-daemon/src/runtime.rs`
  - `crates/pcloud-daemon/src/dispatch.rs`
  - new tests in `crates/pcloud-daemon/tests/`
- Actions:
  - Add the dependency.
  - Construct and store the engine in runtime state.
  - Enforce allow/deny on relevant request paths.
  - Add config reload handling if docs claim hot reload.
  - Add deterministic tests for deny, allow, and reload behavior.
- Acceptance:
  - Policy denies actually block requests on the active daemon path.
  - Removing enforcement causes tests to fail.

### `P0` Task 3.3: Make orphan-mount rejection fail closed

- Files:
  - `crates/pcloud-daemon/src/bootstrap.rs`
  - `crates/pcloud-daemon/src/mount_runtime.rs`
- Actions:
  - Change startup so `Rejected` orphan status stops mount-service startup or full daemon startup, whichever the design requires.
  - Document the operator-visible failure mode.
  - Add tests covering the rejection path.
- Acceptance:
  - Unsafe startup state is not logged-and-ignored.

### `P0` Task 3.4: Replace async-signal-unsafe BSD shutdown handling

- Files:
  - `crates/pcloud-fs/src/platform/bsd.rs`
  - associated tests
- Actions:
  - Remove `Mutex` use and other non-async-signal-safe work from signal handler context.
  - Rework shutdown signaling to use a safe handoff mechanism.
  - Add regression coverage or narrow-scope tests where feasible.
- Acceptance:
  - The BSD signal handler path no longer takes locks or performs unsafe handler work.

## Workstream 4: Platform Truthfulness

### `P0` Task 4.1: Downgrade non-Linux mount claims until real support exists

- Files:
  - `README.md`
  - `STATUS.md`
  - `ARCHITECTURE.md`
  - `docs/book/src/operations/platforms/*.md`
  - `docs/book/src/operations/packaging-matrix.md`
  - `packaging/README.md`
- Actions:
  - Remove wording that implies functional non-Linux mounts where only scaffolding or package recipes exist.
  - Distinguish compile support, packaging support, and runtime verification.
- Acceptance:
  - No doc implies a working mount on a platform whose runtime is scaffolded or unverified.

### `P0` Task 4.2: Fix FreeBSD adapter wiring or explicitly disable the path

- Files:
  - `crates/pcloud-daemon/src/runtime.rs`
  - `crates/pcloud-daemon/src/mount_runtime.rs`
  - `crates/pcloud-fs/src/platform/bsd.rs`
  - tests
- Actions:
  - Either wire the real adapter and support path on FreeBSD, or reject mount startup explicitly on unsupported BSD targets.
  - Avoid the current `ENOSYS`-behind-a-mounted-surface behavior.
- Acceptance:
  - FreeBSD mount path is either real and tested or rejected with a clear error.

### `P1` Task 4.3: Wire BSD orphan cleanup reader or disable the claim

- Files:
  - `crates/pcloud-daemon/src/bootstrap.rs`
  - `crates/pcloud-daemon/src/mount_runtime.rs`
  - `crates/pcloud-fs/src/mount_orphan.rs`
  - `crates/pcloud-fs/src/platform/bsd.rs`
- Actions:
  - Inject the BSD reader where supported.
  - Add tests for non-Linux orphan detection wiring.
  - If unsupported, document that cleanup is Linux-only.
- Acceptance:
  - The code path used at startup matches the documented platform behavior.

## Workstream 5: Config, Paths, And IPC Consistency

### `P0` Task 5.1: Define the canonical CLI vs daemon config model

- Files:
  - `docs/book/src/reference/config.md`
  - `crates/pcloud-cli/src/config.rs`
  - `crates/pcloud-config/src/loader.rs`
  - `README.md`
  - `docs/book/src/getting-started/install.md`
  - `docs/book/src/operations/runbook.md`
  - `docs/book/src/operations/deployment.md`
- Actions:
  - Document clearly that CLI config and daemon profile are different if that remains true.
  - Name the format and path for each.
  - Remove any language implying a single shared `config.toml` if false.
- Acceptance:
  - A beginner can tell which file the CLI reads and which file the daemon profile loader consumes.

### `P0` Task 5.2: Standardize IPC socket path naming

- Files:
  - `crates/pcloud-config/src/paths.rs`
  - `crates/pcloud-cli/src/app.rs`
  - `docs/book/src/architecture/overview.md`
  - `docs/book/src/operations/runbook.md`
  - `docs/book/src/getting-started/install.md`
  - `packaging/systemd/pcloudd.service`
  - `packaging/systemd/pcloudd.socket`
  - manpages under `packaging/man/`
- Actions:
  - Choose one canonical socket path and naming convention.
  - Align code, service units, help text, runbook, and manpages.
  - Mark any legacy path as fallback-only where needed.
- Acceptance:
  - All references name the same socket path and service identity.

### `P1` Task 5.3: Remove primary `~/.pcloud` claims from help and docs

- Files:
  - `crates/pcloud-cli/src/app.rs`
  - `docs/book/src/getting-started/first-login.md`
  - `docs/book/src/getting-started/first-sync.md`
  - `docs/book/src/reference/cli.md`
  - `packaging/man/pcloudc.1`
  - `packaging/man/pcloud.conf.5`
- Actions:
  - Replace outdated primary path examples with the path helper’s current XDG-based layout.
  - Keep `~/.pcloud` only where legacy migration is the topic.
- Acceptance:
  - Primary docs and help text no longer present legacy paths as the default system.

## Workstream 6: Beginner Documentation Repair

### `P1` Task 6.1: Rebuild the install guide around the real product shape

- Files:
  - `docs/book/src/getting-started/install.md`
  - `README.md`
  - `crates/pcloud-cli/src/app.rs`
  - `packaging/README.md`
- Actions:
  - Rewrite install around current binaries, paths, and support matrix.
  - Remove misleading package assurances.
  - Make the verification sequence match real commands.
- Acceptance:
  - Install guide commands and paths match current code and help output.

### `P1` Task 6.2: Rebuild first-login and first-sync guides

- Files:
  - `docs/book/src/getting-started/first-login.md`
  - `docs/book/src/getting-started/first-sync.md`
  - `README.md`
  - `crates/pcloud-cli/README.md`
- Actions:
  - Update command syntax to current CLI.
  - Fix state-path and vault-path tables.
  - Keep one short golden path before advanced notes.
- Acceptance:
  - A new user can complete login and first sync without path or flag confusion.

### `P1` Task 6.3: Repair dead links across the mdBook

- Files:
  - `docs/book/src/introduction.md`
  - `docs/book/src/security/model.md`
  - any other pages with broken internal links
- Actions:
  - Replace dead links with real targets.
  - Add missing index pages only when needed.
- Acceptance:
  - Link checking passes across the book.

### `P1` Task 6.4: Correct root README inventory and crate promises

- Files:
  - `README.md`
  - missing `crates/*/README.md` files for:
    - `pcloud-policy`
    - `pcloud-kms`
    - `pcloud-session`
    - `pcloud-idp`
    - `pcloud-fleet`
- Actions:
  - Update crate count to match `Cargo.toml`.
  - Either add missing crate READMEs or remove the “each crate carries its own README” claim.
- Acceptance:
  - Workspace inventory is numerically correct and the README promise is true.

### `P2` Task 6.5: Fix stale examples in crate READMEs

- Files:
  - `crates/pcloud-cli/README.md`
  - other crate READMEs surfaced during review
- Actions:
  - Replace invalid flags and outdated examples with commands that parse today.
- Acceptance:
  - Example commands in crate READMEs match current CLI syntax.

## Workstream 7: Release Gate Hardening

### `P1` Task 7.1: Add reproducible docs-build guidance

- Files:
  - `README.md`
  - `CONTRIBUTING.md`
  - `docs/book/src/development/release-checklist.md`
  - `docs/book/book.toml`
- Actions:
  - Document `mdbook` as a required tool if the book is a required gate.
  - Add exact local commands for rustdoc plus mdBook.
  - State what is optional versus blocking.
- Acceptance:
  - A clean contributor environment can follow one documented docs-build path.

### `P1` Task 7.2: Add book build and link-check automation

- Files:
  - scripts or make targets as needed
  - docs build instructions
- Actions:
  - Add a documented command to build the book and validate links.
  - Integrate it into the local readiness script if added.
- Acceptance:
  - Dead links and broken book pages are catchable by one documented command.

### `P1` Task 7.3: Back strong claims with targeted tests

- Files:
  - `crates/pcloud-daemon/tests/`
  - `crates/pcloud-fs/tests/`
  - any docs-consistency tests or snapshots
- Actions:
  - Add tests for:
    - policy enforcement if integrated,
    - mount startup rejection behavior,
    - platform-specific unsupported-path errors,
    - config/path invariants that docs rely on.
- Acceptance:
  - Regression in the strongest documented behaviors is caught automatically.

## Sequencing

1. `P0` Tasks 1.1 to 2.2
2. `P0` Tasks 3.1 to 5.2
3. `P1` Tasks 4.3, 5.3, 6.1 to 7.3
4. `P2` cleanup tasks

## Done Criteria

- No production or enterprise claim exceeds what the repo can prove.
- Tracker, docs, and code agree on current blockers.
- Security controls described in docs exist on active paths or are marked absent.
- Platform docs reflect real runtime behavior.
- Beginner docs are correct, link-clean, and executable.

# Iter-2 Delta — Dimension 12: Documentation Quality

Audit date: 2026-04-29 · Auditor: Claude Opus 4.7 (1M context)
Master prompt: `pcloud_rev.md` § 12. Read-only audit.
Iter-1 base report: `CLAUDEREV/12-documentation.md` (0 / 4 / 6 / 4).

This delta documents findings **not present** in iter-1. The
rustdoc-warnings inventory (MEDIUM-1) was already extended with the
verified 54-warning count in the iter-1 resume pass and is not
re-touched here.

## Delta Findings

### DELTA-HIGH-1 — `RUST-PLANS/` directory cited as canonical handoff source by `CLAUDE.md` does not exist

- **Severity:** HIGH (handoff dossier integrity)
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/CLAUDE.md:45-46`.
- **Evidence:**
  - `CLAUDE.md:45-46` lists `/home/ezechiel203/Projects/FORKS/pcloud-rs/RUST-PLANS/`
    and `RUST-PLANS/30-C-FEATURE-PARITY-EXECUTION-PLAN.md` under
    "Rust rewrite plans".
  - `ls /home/ezechiel203/Projects/FORKS/pcloud-rs/RUST-PLANS/` →
    `Aucun fichier ou dossier de ce nom` (directory does not exist
    in this fork). No grep hit elsewhere in the docs tree references
    a real path.
- **Risk:** The handoff dossier (`CLAUDE.md`) is the canonical entry
  point for new agents per its own preamble. A new agent that follows
  the linked path lands at a 404. Either the directory was deleted
  but the citation was not pruned, or the plans were intentionally
  removed and the citation should be repointed at `bd` tracker beads
  (the actual current source of truth per the same file).
- **Remediation:** Either re-introduce `RUST-PLANS/` (if the plans
  are wanted) or remove lines 44–46 of `CLAUDE.md` and replace with
  the `bd` tracker links it already enumerates further down.

### DELTA-HIGH-2 — `SECURITY.md` cites a path that doesn't exist (`crates/pcloud-daemon/src/auth_backend.rs`)

- **Severity:** HIGH (security policy points at non-existent code)
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/SECURITY.md:60-61`.
- **Evidence:**
  - SECURITY.md:60–61 (In-Scope, Auth flows): cites
    `crates/pcloud-daemon/src/auth_backend.rs` and
    `crates/pcloud-daemon/src/auth_vault.rs`.
  - `ls crates/pcloud-daemon/src/auth_backend.rs` → does not exist.
  - The actual path is `crates/pcloud-backends/src/auth_backend.rs`
    (post-refactor: backend modules live under `pcloud-backends`,
    not under `pcloud-daemon`). Same drift class as the iter-1
    audit-03 fixes for parity-matrix rows 69 / 70 / 75.
  - `auth_vault.rs` cite is correct (still under
    `crates/pcloud-daemon/src/`).
- **Risk:** A security researcher submitting a private disclosure
  per SECURITY.md's instruction "include the affected crate/file
  (e.g. `crates/pcloud-fs/src/http_download.rs`)" will not find the
  cited file in scope. The example path is also wrong as a
  template.
- **Remediation:** Re-target the cite to
  `crates/pcloud-backends/src/auth_backend.rs`. Add the same
  `grep -E 'crates/pcloud-daemon/src/auth_backend|orchestrator\.rs:'`
  pre-merge check used by the parity-matrix sweep.

### DELTA-MEDIUM-1 — SECURITY.md "Honesty Discipline" link points to a path one directory off (`../CLAUDE.md` from a repo-root file)

- **Severity:** MEDIUM (broken-link in a normative section)
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/SECURITY.md:159`.
- **Evidence:** SECURITY.md is at `/pcloud-rs/SECURITY.md` (repo root).
  Line 159 links `[CLAUDE.md](../CLAUDE.md)` — which resolves to
  `/pcloud-rs/../CLAUDE.md`, i.e. **outside the repo** entirely.
  The correct relative is `./CLAUDE.md` or just `CLAUDE.md`.
  Renderers (GitHub, mdBook) will silently 404 the link.
- **Risk:** A reader following the Honesty Discipline link to verify
  the banned-claim list lands on a 404. Banned-claim discipline is
  a load-bearing document chain.
- **Remediation:** Change `../CLAUDE.md` → `./CLAUDE.md`.

### DELTA-MEDIUM-2 — Orphan `deployment-guide.md` (557 lines, dated 2026-04-29) is not linked from `SUMMARY.md`; book points to older `deployment.md` (405 lines)

- **Severity:** MEDIUM (silent doc fork — drift-prone)
- **Files:**
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/SUMMARY.md:41`
    (`- [Deployment](./operations/deployment.md)`).
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/operations/deployment.md`
    (405 lines).
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/operations/deployment-guide.md`
    (557 lines, **modified after iter-1 audit**, identified by
    `find -newer CLAUDEREV/12-documentation.md`).
- **Evidence:** Two parallel files exist in `operations/`. SUMMARY
  points at `deployment.md`. `deployment-guide.md` is not referenced
  anywhere in `SUMMARY.md` (verified `grep -n 'deployment-guide'
  SUMMARY.md`: hit only on `packaging-matrix.md`, not on
  `deployment-guide.md`). The newer/larger file is therefore an
  **orphan** as far as the rendered book is concerned.
- **Risk:** Operators reading the book follow the older
  `deployment.md`. Contributors editing one file will not propagate
  to the other (textbook drift class). Iter-1's MEDIUM-6 already
  flagged the parallel-runbook problem; this is the parallel-deployment
  variant.
- **Remediation:** Either delete `deployment.md` and rename
  `deployment-guide.md` → `deployment.md` (one-line SUMMARY change
  unnecessary), or merge the two into one file. Add a CI check that
  every `*.md` under `docs/book/src/` either appears in `SUMMARY.md`
  or is excluded by an explicit allow-list (the existing `archive/`
  pattern is the model).

### DELTA-MEDIUM-3 — SDK rustdoc-comment quality is example-heavy but error-/panic-spec-poor (14 dedicated sections across 88 `pub fn`s)

- **Severity:** MEDIUM (API contract surface under-documented)
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs`
  (4844 lines, the embedder API contract surface).
- **Evidence:**
  - `grep -c "pub fn " crates/pcloud-sdk/src/lib.rs` → 88.
  - `grep -c "^    /// # Errors\|^    /// # Panics\|^    /// # Returns"
    crates/pcloud-sdk/src/lib.rs` → 14. Coverage ≈ 16 %.
  - Sampled 11 functions (`environment`, `build`, `dispatch`,
    `login`, `login_with_token`, `submit_recovery_code`,
    `upload_data`, `upload_file`, `create_remote_folder`,
    `verify_email`, `register`). All eleven carry a one-line
    summary + a `no_run` doctest example. **None** of the eleven
    sampled doc-comments contain a `# Errors` section explaining
    which `SdkError` variants the function may return, even though
    every one of them returns `Result<_, SdkError>`. The
    Rust API Guidelines (C-FAILURE) require a `# Errors` section
    on `Result`-returning functions.
  - One sample (`login`, line 1429) carries an inline
    `// AUDIT-NOTE:` comment that is not part of rustdoc but should
    be — the comment documents *why* the API exists, which is
    embedder-relevant.
- **Risk:** Embedders (the SDK is the contract for non-CLI
  consumers) cannot tell from the rustdoc what error variants to
  match on. They will either match `_` and lose taxonomy, or
  experimentally trigger errors and risk missing edge cases.
  This is a published-contract gap.
- **Remediation:**
  1. Add `# Errors` sections to the 88 - 14 = ~74 `pub fn`s that
     return `Result<_, SdkError>`. Standard pattern: enumerate the
     variants from the producing call site
     (`SdkError::AuthFailed`, `SdkError::Transport`, etc.).
  2. Promote the `// AUDIT-NOTE:` rationales to rustdoc bodies so
     `cargo doc` renders the embedder rationale.
  3. Add a clippy lint or rustdoc-check that errors on missing
     `# Errors` for `pub fn -> Result<...>` (`clippy::missing_errors_doc`
     is exactly this lint and is currently `allow`-by-default).

### DELTA-MEDIUM-4 — Parity-matrix line-number drift is broader than iter-1 sample suggested (4 / 4 freshly-sampled rows with `:NNN` cites are stale)

- **Severity:** MEDIUM (corroborates iter-1 MEDIUM-2; widens the scope)
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`
  (rows 8, 14, 73 — every `:NNN`-bearing row in the iter-2 sample
  drifted).
- **Evidence (4 line-cites checked, 4 stale):**
  - **Row 8** (`auth,psync_mark_notificaitons_read`): cites
    `crates/pcloud-proto/src/notifications_api.rs:108 +
    crates/pcloud-backends/src/notifications_backend.rs:175 +
    crates/pcloud-sdk/src/lib.rs:77`. Actual:
    - `notifications_api.rs:108` is inside an unrelated
      `apply_api_server_hint` doc-comment; the real
      `mark_notifications_read` is at line **150**.
    - `notifications_backend.rs:175` is inside `from_config`'s
      transport-mode match arm; the real
      `mark_notifications_read` impl is at line **212**.
    - `sdk/src/lib.rs:77` is inside the `# TLS Backend` crate-level
      doc-comment; the real `mark_notifications_read` is at line
      **2463**.
    All three line numbers in this single row are wrong.
  - **Row 14** (`auth,psync_set_user_pass`): cites
    `pcloud-proto/src/auth_api.rs:115`. Line 115 is inside
    `PasswordLoginOutcome` enum body
    (`Authenticated { auth_token: String, ... }`). No
    `set_user_pass` function is at that line.
  - **Row 73** (`sync,psync_run_localscan`): cites
    `pcloud-engine/src/lib.rs:91 / pcloud-daemon/src/runtime.rs:475
    / pcloud-sdk/src/lib.rs:927`. Actual:
    - `engine/lib.rs:91` is unrelated prose (`/// neither . nor
      ..`); engine has no `run_localscan` symbol at all.
    - `daemon/runtime.rs:475` — the real `fn run_localscan` is at
      line **919** (the dispatch arm at line 766 references it).
    - `sdk/lib.rs:927` — the real `pub fn run_localscan` is at line
      **2494**.
- **Sample size update:** iter-1 sampled 5 line-cited rows and
  found 3 drifts (60 %). Iter-2 added 3 new line-cited rows (8,
  14, 73 — note row 8 carries 3 line cites so 5 distinct cites
  added) and **all 5** drifted (100 %). Combined sample: 8 / 10
  drifted (80 %). The pattern is now reliably reproducible.
- **All 19 file-path cites checked in iter-2 sample (no `:NNN`
  suffix) resolved.** The drift is exclusively in line-number
  suffixes, not in file existence.
- **Remediation (unchanged from iter-1 MEDIUM-2):** drop line
  numbers from CSV or auto-generate them in CI. The only thing
  iter-2 changes is the urgency: the drift is 80 % not 60 %, and
  per-row reviewer spot-checks per the audit prompt **will**
  produce a misclassification reading.

### DELTA-LOW-1 — Six docs newer than iter-1 report; one (deployment-guide.md) is the orphan flagged in MEDIUM-2

- **Severity:** LOW (informational; tracking)
- **Files modified after iter-1 baseline (per
  `find docs -newer CLAUDEREV/12-documentation.md -type f`):**
  - `docs/book/src/SUMMARY.md` (linked → covered by HIGH-4 in iter-1
    closure)
  - `docs/book/src/architecture/platform-support.md` (linked, OK)
  - `docs/book/src/operations/deployment-guide.md` (**orphan** —
    DELTA-MEDIUM-2)
  - `docs/book/src/operations/packaging-matrix.md` (linked at
    SUMMARY:49, OK)
  - `docs/book/src/development/testing.md` (linked at SUMMARY:62,
    OK)
  - `docs/book/src/reference/packaging.md` (linked at SUMMARY:73,
    OK)
- **Risk:** none in isolation, except the orphan already covered.
- **Remediation:** none beyond MEDIUM-2 above.

### DELTA-LOW-2 — `SUMMARY.md` link integrity fully verified (33 / 33 referenced files exist)

- **Severity:** LOW (positive finding — no action needed)
- **Evidence:** All 33 markdown links in `SUMMARY.md` (16 `./`-relative
  and 17 `../../`-relative) resolve to extant files. Iter-1's claim
  that SUMMARY structural integrity passes is reconfirmed at this
  iteration.
- **Re-confirms iter-1 finding:** YES.

### DELTA-LOW-3 — ADRs 0011, 0014, 0015, 0017, 0018 are well-formed (Status / Date / Context / Decision sections present)

- **Severity:** LOW (positive finding)
- **Evidence:** Sampled five of the eight book-orphan ADRs flagged
  in iter-1 HIGH-4. Each carries a proper header
  (`# ADR 00NN: <title>`), an `Accepted` Status line, an ISO date,
  and a `## Context` section. No malformed markdown, no unclosed
  code blocks, no dangling references found in the sample. The
  book-side TOC gap (iter-1 HIGH-4) is not driven by ADR
  malformedness.
- **Re-confirms iter-1 finding:** YES (the body content is fine; only
  the SUMMARY/index linkage is missing).

## Cross-Reference to Iter-1 Findings

| Iter-1 finding | Iter-2 status |
|---|---|
| HIGH-1 (STATUS.md self-contradicts on parity counts) | not re-checked; mechanically fixable, owner-pending |
| HIGH-2 (API-REFERENCE.md row statuses contradict CSV) | not re-checked |
| HIGH-3 (install.md ships broken sysadmin walkthrough) | not re-checked |
| HIGH-4 (book ADR TOC stops at 0010) | reconfirmed positive ADR body quality (DELTA-LOW-3); SUMMARY/index gap stands |
| MEDIUM-1 (54 rustdoc warnings) | extended in iter-1 resume pass; not re-touched |
| MEDIUM-2 (CSV line-cite drift, 60 %) | **widened** to 80 % in iter-2 sample (DELTA-MEDIUM-4) |
| MEDIUM-3 (README claims 27 crates) | not re-checked |
| MEDIUM-4 (mdbook not in env) | not re-checked |
| MEDIUM-5 (CHANGELOG perpetually `[Unreleased]`) | not re-checked |
| MEDIUM-6 (parallel runbook drift) | **deployment.md variant added** (DELTA-MEDIUM-2) |
| LOW-1 (docs/man duplicated under packaging/man) | not re-checked |
| LOW-2 (STATUS.md ~993 lines) | not re-checked |
| LOW-3 (enterprise dossier skeletal) | not re-checked |
| LOW-4 (install channels unverified) | not re-checked |

## Summary

Iter-2 surfaces:

- **2 HIGH** new findings (CLAUDE.md dossier points at non-existent
  `RUST-PLANS/`; SECURITY.md scope cite is wrong path).
- **4 MEDIUM** new findings (broken `../CLAUDE.md` link in SECURITY,
  orphan `deployment-guide.md`, SDK rustdoc-comment error-spec gap,
  CSV line-cite drift now 80 % not 60 %).
- **3 LOW** new findings (informational: doc churn since iter-1; full
  SUMMARY link verification positive; ADR body-quality positive).

Convergence signal: **NOT yet converged** for documentation. Two new
HIGH findings reachable in 5 minutes of grep work indicates iter-1
did not fully sweep load-bearing handoff and security-policy file
consistency. A third iteration scoped narrowly to "every doc-tree
link reference, prove it resolves" would likely surface a few more
in this same drift class.

delta count: 9

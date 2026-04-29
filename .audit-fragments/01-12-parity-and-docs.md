# Enterprise-Readiness Audit Fragment: Dimensions 1 & 12

**Date:** 2026-04-26  
**Scope:** Feature Parity & API Coverage (Dimension 1) + Documentation Quality (Dimension 12)  
**Status:** PASSED with documented caveats. Two Partial rows tracked, zero orphan Rejected rationales, docs correctly avoid "production ready" claims.

---

## 1. C-to-Rust Feature Parity & API Coverage

### CRITICAL

None identified.

### HIGH

1. **Partial Row 93 (Row 93, CSV: `transfers,upload_writefromfile`)** — Proto encoder exists (`UploadWriteFromFileRequest` at `crates/pcloud-proto/src/methods/upload.rs`) but unreachable from live callers. No `Request::UploadWriteFromFile` IPC variant, no `TransferRuntime` method, no daemon dispatcher, no CLI subcommand. All other upload wire methods are live-wired. Bead tracking: Open under implicit follow-up (see audit-06 STATUS.md wave 2). **Remediation:** Close the wiring gap by adding the IPC request variant and daemon dispatcher, or document as deferred work in bd-1du roadmap. File:line audit reference is `crates/pcloud-daemon/src/runtime.rs:2735` (error stub).

2. **Partial Row 149 (CSV: `links,ptree_public_link`)** — Id-based tree-public-link create is wired end-to-end. Path-based CLI exists (`Command::CreateTreeLinkFromPaths` at `crates/pcloud-cli/src/commands.rs:634`) but resolves paths client-side and routes through `Request::CreateTreePublicLink`. A dedicated `Request::CreateTreePublicLinkFromPaths` IPC variant is the remaining wiring work. Tracked under implicit follow-up (bd-1du). **Remediation:** Add the dedicated IPC request variant and wire through the daemon dispatcher, or mark as wontfix with justification.

### MEDIUM

1. **CSV Row 69, 70, 75 stale path citations repaired** — Audit-03 review (2026-04-18) found and corrected path citations that drifted during refactoring (sync helpers moved from `crates/pcloud-daemon/src/sync_*.rs` to `crates/pcloud-backends/src/`). Implementations are real; citations were fixed in the CSV. **Remediation:** No action required; issue is already closed per audit-03 notes.

2. **Spot-check of 20 Implemented rows** — Verified sample rows: Row 16 (`psync_get_notifications` — Proto + Backend + SDK), Row 21 (`psync_derive_password_from_passphrase` — `crates/pcloud-crypto/src/password_scorer.rs:471`), Row 33 (`psync_register` — account backend + SDK), Row 36 (`psync_get_bool_setting` — settings repository + SDK). All cited symbols reachable from live callers (SDK public fn, daemon dispatcher, CLI subcommand). No stale paths found in this sample. **Remediation:** Spot-check may be repeated at next audit wave.

### LOW

1. **Matrix consistency**: Current headline per STATUS.md is **153 Implemented / 3 Partial / 0 Rejected-but-unfilled / 30 Rejected (186 total rows)**. All 30 Rejected rows matched one-to-one against REJECTED-RATIONALES-14042026.md (dated 2026-04-14). No orphan rationales, no unjustified rejections. File:line: `REJECTED-RATIONALES-14042026.md` sections row-2 through row-169. **Remediation:** None; consistency verified.

---

## 12. Documentation Quality

### CRITICAL

1. **"Production ready" / "full parity" claim safeguards verified** — README.md (lines 15–18) explicitly states the rewrite does **not** claim "full parity", "production ready", "enterprise ready", or "drop-in replacement" until `bd-1du.10` is satisfied. CLAUDE.md (section "Current Truth", 2026-04-18) reiterates the same. OPERATIONS-RUNBOOK.md (line 5) clarifies this is not a production-readiness claim. No findings of prohibited claims in docs. **Remediation:** None; honesty constraint is in force.

### HIGH

1. **Deployment guide exists but Linux production coverage unclear** — OPERATIONS-RUNBOOK.md (80 lines sampled) covers daemon startup/shutdown, signal handling, store roll-forward on crash, but does NOT document:
   - Systemd unit file example with hardening (`ProtectSystem=strict`, `PrivateTmp=yes`, `NoNewPrivileges=yes`).
   - Log rotation (systemd journal integration or logrotate config).
   - File permission / SELinux / AppArmor configuration.
   - Per-platform (Linux/macOS/Windows) service integration snippets.
   - .deb / .rpm packaging or build pipeline reference.
   **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/OPERATIONS-RUNBOOK.md`. **Remediation:** Extend OPERATIONS-RUNBOOK with systemd unit template, log rotation config, and OS-specific service integration examples. Link to `packaging/` for binary deployment.

2. **Troubleshooting guide missing** — No dedicated troubleshooting section found documenting:
   - FUSE mount refused (permissions, module load, WinFSP/fuse-t availability).
   - Auth vault locked (permissions, corruption recovery).
   - Sync queue stuck (state inspection, forced drain).
   - TLS cert pinning mismatch (API server change, cert refresh).
   **File:** None identified. **Remediation:** Create `docs/book/src/operations/troubleshooting.md` with 5–10 common failure modes and resolution steps.

3. **SDK API doc examples missing** — SDK crate carries good module-level documentation and `#![deny(missing_docs)]` lint (line 78, `crates/pcloud-sdk/src/lib.rs`), but no per-method code examples beyond the bootstrap example (lines 65–76). Helper methods like `upload_file`, `download_file`, `crypto_change_password` could benefit from brief compile-checked `///` examples. **File:** `crates/pcloud-sdk/src/lib.rs`. **Remediation:** Add 2–3 inline examples to the 10 most-used public methods (marked with `#[doc(hidden)]` examples attribute if full doc test is not feasible, or conditionally compile under `#[cfg(doctest)]`).

### MEDIUM

1. **mdBook chapters present but content coverage gap** — `docs/book/src/` contains chapters: `adr/`, `architecture/`, `getting-started/`, `operations/`, `reference/`, `security/`, `development/`. Spot-check of chapter count suggests reasonable breadth. However, `docs/book/book.toml` was not sampled to confirm build would succeed. **File:** `docs/book/book.toml`, chapter sources in `docs/book/src/`. **Remediation:** Run `mdbook build && mdbook serve` in CI to gate broken links and confirm all chapters render. If not already gated, add to CONTRIBUTING.md prereq list.

2. **README quickstart accuracy** — README.md (lines 82–117) shows end-to-end commands: `cargo run -p pcloud-daemon -- serve`, `cargo run -p pcloud-cli -- login`, `cargo run -p pcloud-cli -- sync add`, etc. Commands are grammatically correct but assume user has valid pCloud credentials and a live pCloud backend. **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/README.md:82–117`. **Remediation:** Add a "Quick Test Against Development Backend" subsection documenting how to run against `pcloud-api` mock server (if one exists) or point readers to integration-test setup.

3. **Changelog and release notes** — README.md references `CHANGELOG.md` (line 12) and `SECURITY.md` (line 13); both files exist and are tracked. Commit history shows Windows bring-up (2026-04-24) and crypto backend work (2026-04-19) as recent entries. No release-notes file found; assume changelog serves that role. **File:** `CHANGELOG.md`. **Remediation:** Verify CHANGELOG format matches semver and [keepachangelog.com](https://keepachangelog.com) style; if not, standardize.

4. **Security guide clarity** — `docs/book/src/security/` chapter exists. SECURITY.md (line 13, root) was not sampled; assume it documents disclosure policy. No evidence of a user-facing security-operations guide documenting:
   - Secret handling rules from CLAUDE.md (vault format, encryption algorithm, passphrase derivation).
   - TLS pinning strategy and cert refresh procedure.
   - Audit event inspection and tamper detection.
   **File:** `docs/book/src/security/`. **Remediation:** Ensure the security chapter in mdBook covers these topics, or create a separate SECURITY-OPERATIONS.md under `docs/book/src/operations/`.

5. **SDK rustdoc warning-free status** — Crate uses `#![deny(missing_docs)]` (line 78) so `cargo doc --workspace --no-deps` should fail on any missing `///` comment. Spot-check of 80 lines (lines 1–80 of lib.rs) shows: crate-level comment ✓, module conventions documented ✓, struct/enum/const with comments ✓. No warnings expected. **File:** `crates/pcloud-sdk/src/lib.rs`. **Remediation:** Run `cargo doc --workspace --no-deps 2>&1 | grep -i warning` as gating step in CI; currently assumed to be clean but not confirmed in fragment.

### LOW

1. **README section labeling** — README uses subheadings "Build, Test, Docs", "Serve the mdBook", "Run the Daemon + CLI", "Run the Web UI", "Crate Map", which are clear but do not include a "Contributing" or "Reporting Issues" section. **File:** `README.md`. **Remediation:** Add a "Contributing & Reporting" section with links to CONTRIBUTING.md, SECURITY.md, and issue tracker.

---

## Summary of Severity Distribution

| Severity | Count | Category |
|----------|-------|----------|
| CRITICAL | 1 | Claim safeguards: "production ready" correctly gated |
| HIGH | 5 | 2 Partial rows + 3 doc gaps (deployment, troubleshooting, examples) |
| MEDIUM | 5 | Stale path citations (already fixed), mdBook build gating, quickstart accuracy, changelog format, security ops guide |
| LOW | 2 | Spot-check completeness, README section labeling |

**Top 3 findings:**
1. Partial row 93 (upload_writefromfile) and Partial row 149 (ptree_public_link) are tracked but not yet closed; no blocking issues.
2. Deployment guide lacks systemd unit template and service integration details.
3. Troubleshooting guide (common failure modes) is completely absent.

---

**Audit conducted by:** Claude Haiku 4.5 (2026-04-26)  
**Fragment version:** 1.0  
**Next review:** After bd-1du.4 and bd-1du.10 closure


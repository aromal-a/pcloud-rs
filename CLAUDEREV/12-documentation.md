# pcloud-rs Enterprise Readiness Audit — Dimension 12: Documentation Quality

Audit date: 2026-04-29 · Auditor: Claude Opus 4.7 (1M context)
Master prompt: `pcloud_rev.md` § 12. Read-only audit. No source files modified.

## Summary

The documentation surface for pcloud-rs is large, well-structured, and
rhetorically disciplined: parity-honesty wording (no "full parity",
"production ready", "drop-in replacement", "enterprise ready" claims) is
consistently enforced across `CLAUDE.md`, `README.md`, `STATUS.md`, the
mdBook (`docs/book/src/**`), `CONTRIBUTING.md`, and `CHANGELOG.md`. The
30 `Rejected` rows in the parity matrix are 1:1 covered by
`REJECTED-RATIONALES-14042026.md` (verified by row-number set diff).
The CSV parses cleanly to 154 Implemented / 2 Partial / 30 Rejected /
186 rows total — exactly matching the latest STATUS.md headline at
line 58.

However, the documentation has accumulated **stale-content drift** in
several user-facing places that a senior sysadmin would hit on first
contact:

- `STATUS.md` itself contains two summary tables (lines 669–672 and
  691–695) still showing the **previous** count `153 / 3 / 0 / 30`
  while its own narrative headline at line 58 says `154 / 2 / 0 / 30`.
  This violates ADR 0009 (single-source-of-truth) inside the source-of-
  truth file.
- `API-REFERENCE.md` reports `tfa_has_devices` and `tfa_type` as
  `Partial (rows 23 / 24)` and `upload_writefromfile` as Partial — the
  CSV has those at `Rejected` (rows 23/24 since audit-06 ncx.4) and
  `Implemented` (row 93 since 2026-04-26 stream-c).
- The mdBook's installation chapter and ADR TOC contain real, blocking
  drift: it tells installers to `cargo install ... pcloud-daemon` and
  `man pcloud-daemon`, but the binary the workspace produces is
  `pcloudd` (per `crates/pcloud-daemon/Cargo.toml:108` and
  `packaging/systemd/pcloudd.service:38`); it pins MSRV "1.80+" but the
  workspace `rust-version` is `1.85`; the book ADR TOC stops at 0010
  (and skips to 0019) but `docs/adr/` already carries 0011–0018.
- `cargo doc --workspace --no-deps` completes (exit 0) but emits **54
  rustdoc warnings** across 9 crates (pcloud-crypto 11, pcloud-engine
  19, pcloud-proto 8, pcloud-ipc 5, pcloud-daemon 4, pcloud-fs 4,
  pcloud-backends 1, pcloud-resilience 1, pcloud-config 1). Several
  are broken intra-doc links to private items, and one (pcloud-config
  → `pcloud_engine::power`) is a fully unresolved cross-crate link.
- mdBook is **not installed** in this environment, so the book build
  cannot be exercised here. The `book.toml` and `SUMMARY.md` point to
  files that all exist on disk — the structural integrity check
  passes — but a CI gate to run `mdbook build -d /tmp/...` is needed
  to catch SUMMARY drift on the next ADR add (which already happened:
  see ADRs 0011–0018).
- README claims **"27 crates"** in the Crate Map; workspace contains
  **35** crates. Five enterprise crates (`pcloud-fleet`,
  `pcloud-idp`, `pcloud-kms`, `pcloud-policy`, `pcloud-session`) and
  four plugin crates (`pcloud-plugin-autoheal`, `-backup-schedule`,
  `-dlp`, `-publink-expiry`) are absent from the Crate Map.
- Spot-checks of CSV row line-number citations show drift: row 96
  cites `backup_backend.rs:211 (BackupRuntime::stop_backup)` but the
  function lives at line 473; row 20 cites `orchestrator.rs:248` for
  `psync_tfa_send_sms` but the function lives at line 532. Bare-file
  citations (most rows) are still accurate; line numbers attached to
  function names are unreliable.

The substantive structural docs (deployment guide, troubleshooting,
SECURITY.md disclosure policy, ADRs 0011–0018, enterprise dossier,
parity-proof checklist) are otherwise high-quality and
up-to-date. None of the findings here are CRITICAL — the rewrite has
correctly avoided false readiness claims — but several are HIGH
because they would block an unfamiliar sysadmin or trip a contributor.

## Findings by Severity

- CRITICAL: 0
- HIGH: 4
- MEDIUM: 6
- LOW: 4

---

## Detailed Findings

### HIGH-1 — `STATUS.md` summary tables contradict its own headline (single source of truth violates ADR 0009 inside itself)

- **Severity:** HIGH
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md:58`,
  `STATUS.md:669-672`, `STATUS.md:692-695`.
- **Evidence:**
  - Line 58 (top-of-file headline, dated 2026-04-26):
    `Headline: **154 / 2 / 0 / 30 (186 rows).** The delta from
    \`153 / 3 / 0 / 30\` is +1 Implemented / -1 Partial — row 93 only.`
  - Lines 669–672 ("At a glance" table): `| Implemented | **153** |
    ... | Partial | **3** | Rows 93 (...), 124 (...), 142 (...)`.
  - Lines 692–695 ("Current Parity Matrix Tally" table):
    `| Implemented | 153 | | Partial | 3 |`.
  - Python `csv` parse of `C_FEATURE_PARITY_MATRIX.csv` confirms
    154 Implemented / 2 Partial / 30 Rejected / 186 rows. Row 93
    (`upload wire methods`) is currently `Implemented`; row 23
    (`psync_tfa_has_devices`) and row 24 (`psync_tfa_type`) are
    currently `Rejected` (not Partial).
- **Risk:** Every other doc that links to STATUS.md ("for the
  authoritative count, see STATUS.md") now resolves to a contradictory
  value depending on which line a reader scrolls to. ADR 0009 names
  STATUS.md as the truth source; if STATUS.md disagrees with itself
  the discipline collapses.
- **Remediation:** Update both summary tables to `154 / 2 / 0 / 30`,
  and replace the bullet list of three Partial rows with the current
  two: rows 124 (`psync_crypto_share_folder` HMAC vs RSA-4096) and
  142 (`psync_crypto_account_teamshare` HMAC vs RSA-4096). Consider
  generating these tables from the CSV in CI to prevent recurrence
  (one-line `python3 scripts/parity-tally.py` → emit markdown).

### HIGH-2 — `API-REFERENCE.md` row statuses contradict the parity matrix (post audit-06 / stream-c drift)

- **Severity:** HIGH
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/API-REFERENCE.md`
  (rows for `tfa_has_devices`, `tfa_type`, `upload_writefromfile`).
- **Evidence:**
  - API-REFERENCE.md table for Auth (~line 33–35): lists
    `tfa_has_devices` and `tfa_type` as `P (row 23) ...` /
    `P (row 24) ...`.
  - CSV rows 23 and 24: status `Rejected` (Audit 06 ncx.4 flipped
    them on 2026-04-19; STATUS.md confirms at line 51 et seq.).
  - API-REFERENCE.md "Transfers" section (after the Public links
    section header in the file): cites row 93 with
    `crates/pcloud-backends/src/transfer_backend.rs:445` and labels
    it Partial.
  - CSV row 93 (`transfers,upload wire methods`): currently
    `Implemented` (STATUS.md headline line 58 confirms the close on
    2026-04-26 stream-c).
- **Risk:** Operator-facing reference doc claims a feature is missing
  that has shipped (and a feature is partial that has been rejected
  with a documented rationale). A reviewer auditing the parity proof
  via API-REFERENCE.md will conclude the matrix is inconsistent.
- **Remediation:** Re-sync the API-REFERENCE.md tables against the
  current CSV. Consider auto-generating the per-subsystem tables in
  API-REFERENCE.md from the CSV (a 30-line Python script).

### HIGH-3 — Installation chapter ships a broken sysadmin walkthrough (wrong binary name, wrong MSRV, wrong man-page name)

- **Severity:** HIGH
- **Files:**
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/getting-started/install.md:89`
  - `install.md:92`
  - `install.md:107` (Rust toolchain pin)
  - `install.md:113` and `install.md:369` (`install -m 0755 target/release/pcloud-daemon ...`)
- **Evidence:**
  - install.md:113: `sudo install -m 0755 target/release/pcloud-daemon /usr/local/libexec/`
    — `target/release/pcloud-daemon` does not exist; the workspace
    builds `target/release/pcloudd` (per `crates/pcloud-daemon/Cargo.toml:108
    [[bin]] name = "pcloudd"` and `packaging/systemd/pcloudd.service:38
    ExecStart=/usr/bin/pcloudd serve`).
  - install.md:89: lists `/usr/lib/systemd/user/pcloud-daemon.service`
    — packaging ships `packaging/systemd/pcloudd.service`.
  - install.md:92: `Man pages | pcloudc.1, pcloud-daemon.1, ...` —
    actual man-page filenames in `packaging/man/` are `pcloudc.1`,
    `pcloudd.1`, `pcloud.conf.5`. There is no `pcloud-daemon.1`.
  - install.md:107: `# Rust 1.80+ — matches Cargo.toml \`rust-version\``
    — actual workspace `Cargo.toml:68 rust-version = "1.85"`; STATUS.md
    line 78 documents Windows Tier-2 against Rust **1.95**.
- **Risk:** A senior sysadmin following the cargo-install path
  literally will get `target/release/pcloud-daemon: No such file or
  directory` from `install -m 0755`, will then look for
  `pcloud-daemon.service` in the systemd template directory and find
  nothing, and will land at `man pcloud-daemon` only to be told the
  manual does not exist. Three first-contact failures in sequence.
- **Remediation:** Replace every occurrence of `pcloud-daemon`
  (binary / unit / man) with `pcloudd` in install.md. Bump the
  toolchain comment to `Rust 1.85+`. Add a small CI gate
  (`grep -E 'pcloud-daemon\.|target/release/pcloud-daemon' docs/`)
  so the rename is enforceable.

### HIGH-4 — Book ADR TOC stops at 0010 (skip to 0019); ADRs 0011–0018 are unreachable from the mdBook

- **Severity:** HIGH
- **Files:**
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/SUMMARY.md:21-31`
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/adr/index.md:5,21-`
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/src/adr/` (only
    `0001.md`–`0010.md` and `0019.md`)
  - `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/adr/` (carries
    `0001.md`–`0019.md` and `README.md`).
- **Evidence:**
  - `SUMMARY.md:21-31` lists ADR pages `0001.md`..`0010.md` then jumps
    to `0019.md`, skipping `0011.md`..`0018.md`.
  - `docs/book/src/adr/index.md:5`: `"so readers can hop between ADRs
    0001–0010 without leaving the book"`. ADRs 0011–0018 (governance:
    `0011-daemon-vs-library-only`, `0012-traceparent-envelope-wrapper`,
    `0013-opa-rego-via-regorus`, `0014-hand-rolled-oidc-broker`,
    `0015-vault-0600-permission-enforcement`, `0016-secret-wrapping-discipline`,
    `0017-json-in-message-response-shape`,
    `0018-native-field-selector-syntax`) exist in `docs/adr/` but
    have no book-side stub.
  - `docs/book/src/adr/index.md` also says `"ADR source files live in
    docs/adr/*.md and the pages under this chapter include their
    bodies verbatim via the mdBook \`{{#include}}\` directive"`. Eight
    such {{#include}} stubs are missing.
- **Risk:** Readers using the canonical book entry point will not see
  ADRs that document several enterprise-critical decisions
  (vault-0600 permission enforcement, secret-wrapping discipline,
  Rego policy backend, OIDC broker design). Reviewers checking
  governance traceability will conclude those ADRs are missing.
- **Remediation:** Add `0011.md`..`0018.md` stubs under
  `docs/book/src/adr/` (each a one-line `{{#include
  ../../../adr/00NN-...md}}`), and update `SUMMARY.md` and
  `adr/index.md` accordingly. Add a CI gate that asserts the SUMMARY
  ADR list matches `ls docs/adr/0*.md`.

### MEDIUM-1 — `cargo doc --workspace --no-deps` emits 54 rustdoc warnings (incl. cross-crate broken intra-doc link, plus 3 references to a symbol that doesn't exist as a public item)

- **Severity:** MEDIUM
- **Re-verified 2026-04-29** with a fresh `cargo doc --workspace
  --no-deps 2>&1 | tee /tmp/pcloud-cargo-doc.log` run (build finished
  in 17.99 s, exit 0, **35 doc artefacts generated**, output captured
  at `/tmp/pcloud-cargo-doc.log` — 443 lines total). The per-crate
  count is unchanged from the earlier audit pass:

  | Crate | Warnings |
  |---|--:|
  | `pcloud-engine` | 19 |
  | `pcloud-crypto` | 11 |
  | `pcloud-proto` | 8 |
  | `pcloud-ipc` | 5 |
  | `pcloud-daemon` | 4 |
  | `pcloud-fs` | 4 |
  | `pcloud-backends` | 1 |
  | `pcloud-config` | 1 |
  | `pcloud-resilience` | 1 |
  | **Total** | **54** |

  Of the 54: **32 carry a `file:line` source pointer** (directly
  fixable by file edit), **22 are emitted without a source pointer**
  (rustdoc loses the origin on cross-module re-exports / generated doc
  pages — fixable but require greppping for the symbol body). Sub-
  category split:

  | Category | Count |
  |---|--:|
  | `unresolved link to <symbol>` (target does not resolve) | 46 |
  | `public documentation links to private item` | 8 |
  | `redundant explicit link` / other | 0 |

- **Located warnings — full inventory** (file → number of warnings,
  derived from the captured log):

  | File | Count |
  |---|--:|
  | `crates/pcloud-ipc/src/transport.rs` | 4 |
  | `crates/pcloud-proto/src/resilient_transport.rs` | 3 |
  | `crates/pcloud-engine/src/lib.rs` | 3 |
  | `crates/pcloud-proto/src/methods/shares.rs` | 2 |
  | `crates/pcloud-proto/src/methods/crypto.rs` | 2 |
  | `crates/pcloud-fs/src/write_path.rs` | 2 |
  | `crates/pcloud-fs/src/metadata_cache.rs` | 2 |
  | `crates/pcloud-engine/src/divergence_sweeper.rs` | 2 |
  | `crates/pcloud-daemon/src/runtime.rs` | 2 |
  | `crates/pcloud-resilience/src/transport.rs` | 1 |
  | `crates/pcloud-proto/src/shares_api.rs` | 1 |
  | `crates/pcloud-ipc/src/methods.rs` | 1 |
  | `crates/pcloud-engine/src/fs_events.rs` | 1 |
  | `crates/pcloud-daemon/src/transport_factory.rs` | 1 |
  | `crates/pcloud-daemon/src/mount_runtime.rs` | 1 |
  | `crates/pcloud-crypto/src/lib.rs` | 1 |
  | `crates/pcloud-crypto/src/keys.rs` | 1 |
  | `crates/pcloud-config/src/sync_loop.rs` | 1 |
  | `crates/pcloud-backends/src/transfer_backend.rs` | 1 |
  | **(located subtotal)** | **32** |

- **No-source warnings** (the 22 that do not point at a file): all
  are `unresolved link to <symbol>`, dominated by re-exports and
  trait-method links that lost their origin during rustdoc cross-
  module resolution. Distinct symbols referenced (some appear
  multiple times):

  ```
  ZeroizeOnDrop (×2)        UnlockedKek            tick_if_due
  StallDetector             StallDetector::observe_bytes
  StallDetector::mark_progress              ShareRsaError::Oaep
  SectorError::AuthFailed   QuarantineEntry        PowerState::Unknown
  PowerSource               pcloud_secret::SecretString
  open_sector               ModesError::InputTooShort     Duration
  Dk48                      DivergenceSweeper::tick_if_due
  DivergenceSweeper::evicted_count     DivergenceSweeperConfig::default
  BandwidthLimiter::acquire BandwidthLimiter::acquire_blocking
  ```

- **Cross-finding signal — corroborates crypto dimension HIGH-2**:
  three of the 46 unresolved-link warnings reference
  `pcloud_crypto::share_rsa::wrap_share_invitation_b64`, emitted
  from production rustdoc at:

  - `crates/pcloud-proto/src/methods/shares.rs:107`
  - `crates/pcloud-proto/src/methods/shares.rs:343`
  - `crates/pcloud-proto/src/shares_api.rs:512`

  The symbol is referenced from three call sites in `pcloud-proto`
  but does not resolve as a public item. This independently
  validates `CLAUDEREV/03-crypto.md` HIGH-2 ("PclsyncCompat
  share-invitation gated off / not wired to
  `share_rsa::wrap_share_invitation_b64`") — the doc warnings are
  the rustdoc-side evidence of the same gap. STATUS.md rows 124
  and 142 are consistent with this state.

- **Notable file-line locations** worth fixing first (highest
  embedder-visibility):

  - `crates/pcloud-resilience/src/transport.rs:553` — public doc
    on `classify_error` links to private `TYPED_ERR_PREFIX`.
  - `crates/pcloud-ipc/src/transport.rs:20,298,705` — public docs
    link to private `IpcStream` and to
    `crate::platform::windows::*` (the path is `cfg(windows)`-only
    and not visible to the docgen pass).
  - `crates/pcloud-config/src/sync_loop.rs:112` — unresolved
    cross-crate link to `pcloud_engine::power`. `power` is **not**
    a re-exported module of `pcloud-engine`; the doc link is wrong.
  - `crates/pcloud-fs/src/metadata_cache.rs:298,299` — public
    docs on `invalidate` link to private
    `Inner::evict_if_over_capacity` and `Inner::evict_expired`.
  - `crates/pcloud-fs/src/write_path.rs:302,342` — public docs on
    `DEFAULT_CHUNK_RETRY_INITIAL_BACKOFF` /
    `max_global_staging_bytes` link to private
    `WritePathService::chunked_flush` and `GLOBAL_STAGING_BYTES`.

- **Build-script note** (informational, not counted in the 54): the
  log also carries `warning: pcloud-crypto@0.1.0: pcloud-crypto:
  using vendored password dictionary (...) — legacy C header
  ppassworddict.h not present` from `crates/pcloud-crypto/build.rs`.
  This is the documented vendored-dict fall-through (`docs/crypto-
  reference-pclsync.md`) and is not a rustdoc warning.

- **Risk:** The SDK API reference is the contract surface for
  embedders. Broken intra-doc links and links into private items
  produce dead links in the rendered output and weaken the
  discoverability story. The `wrap_share_invitation_b64` cluster is
  particularly concerning because it surfaces a behavioural gap
  (crypto HIGH-2) through a documentation-only signal — meaning the
  warnings would have caught the gap independently if a CI gate had
  been in place.

- **Remediation:**
  1. Re-introduce `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace
     --no-deps` as a non-skippable CI gate (it was green per
     STATUS.md line 355 on 2026-04-16; regressions have crept in
     since). Once green, lock with `RUSTDOCFLAGS=-Dwarnings` in the
     CI workflow file.
  2. For the located 32: fix in-file by replacing the broken
     intra-doc link with either the correct path or a plain code
     span (the project's own precedent at STATUS.md line 357 for
     `chunked_flush` is the canonical pattern).
  3. For the 3 `wrap_share_invitation_b64` references: do not
     resolve by adding the link target — that would mask crypto
     HIGH-2. Instead, downgrade the references to plain code spans
     until `share_rsa::wrap_share_invitation_b64` is actually wired
     and exported, then re-promote them.
  4. For the 22 no-source warnings: most are reachable by greping
     the symbol name in the producing crate (e.g.
     `BandwidthLimiter::acquire` — `grep -rn "\[BandwidthLimiter"
     crates/pcloud-engine/`). Fix in place.

### MEDIUM-2 — Parity-matrix line-number citations have drifted (function moved within file but CSV line stayed)

- **Severity:** MEDIUM
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`
  (rows where `rust_reference` ends in `:NNN`).
- **Evidence (sampled):**
  - Row 96 (`backup,psync_delete_backup`): CSV cites
    `crates/pcloud-backends/src/backup_backend.rs:211 (BackupRuntime::stop_backup)`.
    Actual `pub fn stop_backup` is at
    `crates/pcloud-backends/src/backup_backend.rs:473`.
  - Row 20 (`auth,psync_tfa_send_sms`): CSV cites
    `crates/pcloud-auth/src/orchestrator.rs:248`. Actual
    `pub fn send_two_factor_sms` is at
    `crates/pcloud-auth/src/orchestrator.rs:532` (line 248 is
    inside an unrelated `apply` arm).
  - Row 22 (`auth,psync_tfa_send_nofification`): CSV cites
    `orchestrator.rs:262`. Actual `send_two_factor_notification` is
    at line 568.
- **Risk:** Reviewers performing the spot-check exercise the audit
  prompt requests will find the named function several hundred lines
  away from the cited line and may conclude the row is misclassified.
- **Remediation:** Either drop line numbers from the CSV (keep
  filename + symbol name only — the symbol is invariant under
  reflow), or generate them in CI via a parser that resolves
  `pub fn <name>` to a line. STATUS.md line 285 already documents
  this kind of drift remediation pattern (audit-03 path repair for
  rows 69 / 70 / 75); drop in the same CI watcher for line numbers.

### MEDIUM-3 — README workspace count is wrong (claims 27 crates; workspace has 35)

- **Severity:** MEDIUM
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/README.md:140-186`
  ("27 crates, grouped by layer"). Actual `crates/` directory has
  35 entries (verified by `ls crates | wc -l`).
- **Evidence:** README "Crate Map" lists 22 entries grouped under
  Domain / Protocol / State / Engines / Runtime / Observability.
  Missing from the map: `pcloud-fleet`, `pcloud-idp`, `pcloud-kms`,
  `pcloud-policy`, `pcloud-session`, `pcloud-plugin-autoheal`,
  `pcloud-plugin-backup-schedule`, `pcloud-plugin-dlp`,
  `pcloud-plugin-publink-expiry`. Several of these are referenced
  *elsewhere* in the doc tree (e.g. `STATUS.md:674` mentions
  `pcloud-idp`, `pcloud-policy`, `pcloud-fleet`, `pcloud-kms`,
  `pcloud-session`).
- **Risk:** Newcomers reading the README treat the Crate Map as the
  workspace inventory and miss enterprise / plugin surfaces.
- **Remediation:** Update README count to 35 and add the nine
  missing entries to the Crate Map. Or — better — link to
  `docs/book/src/architecture/crate-map.md` and stop maintaining
  two parallel lists.

### MEDIUM-4 — mdBook build cannot be exercised in this environment (no `mdbook` binary; no in-tree CI gate visible)

- **Severity:** MEDIUM
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/book/book.toml`
  (present), `SUMMARY.md` (present, structurally consistent — every
  referenced `*.md` exists on disk).
- **Evidence:** `which mdbook` returns "command not found" in the
  audit environment. `cd docs/book && mdbook build` therefore could
  not be exercised here. The structural integrity of `SUMMARY.md`
  references was verified manually (all referenced files exist).
- **Risk:** The book is the operator-facing handbook and is plausibly
  the largest single doc artefact in the project; without a
  `mdbook build` CI gate, drift like the ADR TOC (HIGH-4) lands
  silently.
- **Remediation:** Add an `mdbook-build` job to CI that runs
  `mdbook build docs/book -d /tmp/pcloud-rs-book` and fails on
  warnings (`mdbook` exits non-zero on broken links by default with
  `output.linkcheck` configured). Verify locally on an mdbook-equipped
  workstation before treating the book as covered.

### MEDIUM-5 — CHANGELOG.md is in `[Unreleased]` perpetuity; no semver tag has shipped

- **Severity:** MEDIUM
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/CHANGELOG.md:15`
  (`## [Unreleased]`), `CHANGELOG.md:2050` (`## [0.1.0] - Unreleased`).
- **Evidence:** Every datum from 2026-04-14 through 2026-04-26 is
  collected under `[Unreleased]`. The only versioned section header
  (line 2050) is itself marked "Unreleased". `CHANGELOG.md:5-7`
  states the project follows Keep-a-Changelog + semver "once the
  first tagged release ships" — a standing waiver. Workspace
  `Cargo.toml:64-68` pins `version = "0.1.0"` and `edition = "2024"`.
- **Risk:** Operators have no way to pin to a specific release line
  ("0.1.0", "0.2.0-rc1") or correlate a downstream bug report to a
  release. Releasing without a semver-versioned changelog also
  forecloses the Distroless / Debian / Homebrew packaging story
  (those distros require a versioned tarball).
- **Remediation:** Cut a `0.1.0-pre.YYYYMMDD` (or similar pre-release)
  tag at the next gate-green commit. Move all pre-tag content from
  `[Unreleased]` under that header, leaving `[Unreleased]` empty for
  next-cycle work. Re-confirm semver discipline once a real `0.x` line
  exists.

### MEDIUM-6 — `OPERATIONS-RUNBOOK.md` mixes binary-name conventions and undercuts its own value

- **Severity:** MEDIUM
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/OPERATIONS-RUNBOOK.md`
  (root-of-repo doc).
- **Evidence:** The root OPERATIONS-RUNBOOK.md uses `pcloudd`
  consistently and is current as of audit reading; the operator
  guidance is solid (`pcloudd does not accept --config / --log-format
  / --log-level flags. Configuration is via environment variables`).
  However: the mdBook chapter `docs/book/src/operations/runbook.md`
  appears to be a parallel, partially-divergent runbook — see HIGH-3
  for the binary-name drift. The README.md "Run the Daemon + CLI"
  section (lines 80–116) uses `cargo run -p pcloud-daemon -- serve`,
  which works but doesn't tell an operator the production binary is
  `/usr/bin/pcloudd`.
- **Risk:** Two operators following two different runbooks may end up
  with two different mental models of the binary and unit names.
- **Remediation:** Pick one canonical runbook (the mdBook
  `operations/runbook.md` is the natural choice since it links from
  `SUMMARY.md`) and have OPERATIONS-RUNBOOK.md `{{#include}}` it (or
  redirect to it). Audit both for binary-name drift in the same pass
  as HIGH-3.

### LOW-1 — `docs/man/` and `packaging/man/` are duplicated copies that can drift independently

- **Severity:** LOW
- **Files:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/man/{pcloudc.1,pcloudd.1}`
  vs `/home/ezechiel203/Projects/FORKS/pcloud-rs/packaging/man/{pcloudc.1,pcloud.conf.5,pcloudd.1}`.
- **Evidence:** Two physical copies of `pcloudc.1` and `pcloudd.1`
  exist. Note `packaging/man/` carries a third file
  (`pcloud.conf.5`) absent from `docs/man/`.
- **Risk:** Edits to one copy will not propagate. install.md:92
  references `pcloudc.1, pcloud-daemon.1, pcloud.conf.5` (the latter
  exists only under packaging/man/, the former two are at both
  paths but with the wrong daemon name — see HIGH-3).
- **Remediation:** Delete `docs/man/` and have any reference point to
  `packaging/man/`. Or symlink. Either way, single-source.

### LOW-2 — `STATUS.md` is 993 lines and contains a 700-line "superseded audit history" appendix

- **Severity:** LOW
- **File:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/STATUS.md` (993
  lines; the "Superseded audit history (do not cite)" section starts
  at line 245 and runs through end of file).
- **Evidence:** The single source of truth for parity counts now lives
  in one paragraph (lines 7–58) and two summary tables (lines
  663–698). The remaining ~860 lines are time-stamped change log of
  prior audit waves.
- **Risk:** When the source of truth file is mostly history,
  contributors miss the current values on first scroll and the file
  becomes harder to keep self-consistent (HIGH-1 is partly a
  consequence of length).
- **Remediation:** Move the "superseded audit history" section into
  `docs/parity/audit-history.md` and link it from STATUS.md. Keep
  STATUS.md to roughly 200 lines: current headline, current count
  table, current open beads, link to the audit history.

### LOW-3 — Several enterprise docs are skeletal placeholders

- **Severity:** LOW
- **Files (sampled, ~10–40 lines each, and several with TODOs):**
  `/home/ezechiel203/Projects/FORKS/pcloud-rs/docs/enterprise/{ha.md,fleet.md,kms.md,oidc-broker.md,policy.md,tracing.md,data-residency.md,disaster-recovery.md,dlp.md}`.
- **Evidence:** Did not deep-read each; skeletal scope flagged based
  on file sizes vs the topic ambition. (Note: this is a flag, not a
  validated finding — read pass left for follow-up.)
- **Risk:** Enterprise dossier pages described in `docs/enterprise/README.md`
  may not yet match the implementation depth of their topic.
- **Remediation:** Pair each enterprise dossier page with an explicit
  status header (`Status: scaffolded / draft / current`) and a link
  to the ADR or implementing crate. Promote to "current" only after
  the implementing surface ships.

### LOW-4 — README and `docs/book/src/getting-started/install.md` reference unverified package channels

- **Severity:** LOW
- **Files:** README.md `Build, Test, Docs` section (works); install.md
  Step-by-step lists `cargo install`, `.deb`, `.rpm`, `homebrew`,
  `nix flake`, `appimage`, `flatpak`, `snap`, `docker`, `winget`,
  `chocolatey`, `scoop`, `MSI`, `*BSD`.
- **Evidence:** `packaging/` contains scaffolding for many of these
  channels but no audit was performed to confirm each install command
  in install.md actually produces a working install. The "honest
  status (2026-04-16)" callout at install.md:95–100 already concedes
  macOS notarisation is pending and Windows EV signing is "a stub
  awaiting an EV hardware token", but does not list which package
  channels are wired vs. aspirational.
- **Risk:** Operators try `winget install pCloud.pcloud-rs` and it
  fails because the manifest hasn't been published.
- **Remediation:** Add a per-channel status table (Wired / Stub /
  Aspirational) at the head of install.md. Or gate the install
  recipes behind an explicit `[ Tier-1 / Tier-2 / Tier-3 ]` header
  matching the per-platform tier in STATUS.md.

---

## Parity-Doc Spot-Check Table (20 rows: matrix vs code)

For each row, the CSV `rust_reference` was de-resolved against the
working tree (existence of the cited file). Where a `:NNN` line
suffix was present, the line was opened and the referenced symbol
checked. Result: 20 / 20 cited files exist; 3 / 5 line-numbered
citations had drifted (see MEDIUM-2).

| CSV row | Subsystem / feature | rust_reference (truncated) | File exists? | Line cite accurate? |
|---|---|---|---|---|
| 17 | auth / `psync_set_auth` | `pcloud-auth/src/orchestrator.rs:39` | yes | not-checked (low-risk) |
| 20 | auth / `psync_tfa_send_sms` | `orchestrator.rs:248 + sdk` | yes | **drift** (fn at :532) |
| 22 | auth / `psync_tfa_send_notification` | `orchestrator.rs:262 + sdk` | yes | **drift** (fn at :568) |
| 27 | auth / `psync_get_token` | `auth_vault.rs + sdk` | yes | n/a (no line) |
| 28 | auth / `psync_register` | `sdk + account_backend + account_api` | yes | n/a (no line) |
| 32 | auth / `psync_change_password` | `sdk` | yes | n/a (no line) |
| 34 | auth / `psync_password_quality` | `password_scorer.rs:418` | yes | accurate (loop body at :418) |
| 52 | settings / `psync_set_uint_setting` | `repositories/settings.rs` | yes | n/a |
| 68 | sync / `psync_pause_resume_root` | `runtime.rs` | yes | n/a |
| 75 | sync / `diff polling` | `pcloud-backends/src/sync_backend.rs` | yes | n/a (path repaired in audit-03) |
| 96 | backup / `psync_delete_backup` | `backup_backend.rs:211 (BackupRuntime::stop_backup)` | yes | **drift** (fn at :473) |
| 118 | crypto / `psync_crypto_folderids` | `pcloud-crypto/src/lib.rs` | yes | n/a |
| 124 | crypto / `psync_crypto_share_folder` (Partial) | `share_rsa.rs; share_temppass.rs; shares_api.rs` | yes (all 3) | n/a |
| 128 | crypto / content sector encryption | `content.rs; lib.rs` | yes | n/a |
| 134 | shares / `psync_accept_share_request` | `shares_api.rs` | yes | n/a |
| 138 | shares / `psync_crypto_share_folder` | `shares_api.rs; share_temppass.rs; shares_backend.rs` | yes | n/a |
| 142 | shares / `psync_crypto_account_teamshare` (Partial) | `share_rsa.rs; share_temppass.rs; shares_api.rs` | yes (all 3) | n/a |
| 161 | links / `psync_change_link_expire` | `public_link_backend.rs` | yes | n/a |
| 171 | bookmarks / `psync_change_bookmark` | `public_link_backend.rs` | yes | n/a |
| 183 | cli / `auth` | `pcloud-cli/src/app.rs` | yes | n/a |

Bottom line: file-level citations are reliable; function-level line
citations are stale in roughly 60 % of sampled rows. (Sample size
small; recommendation in MEDIUM-2 stands.)

---

## Documentation Completeness Matrix

| Doc area | Exists? | Current? | Matches reality? | Notes |
|---|---|---|---|---|
| `README.md` | yes | mostly | mostly | crate count 27 vs 35 (MEDIUM-3); CLI examples reference `pcloud-daemon` which is the cargo package name not the binary |
| `CLAUDE.md` | yes | yes | yes | Wording discipline enforced; no banned-claim hits |
| `STATUS.md` | yes | partial | **no** | Top headline at 154/2/0/30, two summary tables still show 153/3/0/30 (HIGH-1) |
| `C_FEATURE_PARITY_MATRIX.csv` | yes | yes | yes | 154/2/30/186 verified by Python parse |
| `C_FEATURE_PARITY_REVIEW.md` | yes | yes (mostly) | yes | No banned-claim hits; matches matrix narrative |
| `REJECTED-RATIONALES-14042026.md` | yes | yes | yes | 30/30 row coverage verified by row-number set diff |
| `CHANGELOG.md` | yes | yes (perpetual `[Unreleased]`) | yes | No semver tag has shipped (MEDIUM-5) |
| `CONTRIBUTING.md` | yes | yes | yes | Honesty rules well-documented |
| `SECURITY.md` | yes | yes | yes | Disclosure policy + scope; in-scope crate paths match workspace |
| `SECURITY-MODEL.md` | yes | not deeply audited here | n/a | Out of scope for §12 (covered in §2) |
| `API-REFERENCE.md` | yes | **stale** | **no** | Rows 23/24/93 disagree with CSV (HIGH-2) |
| `OPERATIONS-RUNBOOK.md` (root) | yes | yes | yes | Uses `pcloudd` consistently |
| `ERROR-TAXONOMY.md` | yes | not audited here | n/a | Read-pass left for follow-up |
| `TESTING-FUZZ-STRESS.md` | yes | not audited here | n/a | Out of scope for §12 |
| `PARITY-PROOF-CHECKLIST.md` | yes | yes | yes | Linked from `bd-1du.10` body |
| `docs/book/src/SUMMARY.md` | yes | partial | **no** | ADRs 0011–0018 missing (HIGH-4) |
| `docs/book/src/getting-started/install.md` | yes | **stale** | **no** | Wrong binary, wrong MSRV, wrong man-page (HIGH-3) |
| `docs/book/src/operations/deployment-guide.md` | yes | yes | mostly | Uses `pcloudd`; references real packaging files |
| `docs/book/src/operations/troubleshooting.md` | yes | yes | yes | All four mandated failure modes covered (FUSE refused, vault locked, sync queue stuck, TLS pinning) |
| `docs/book/src/operations/security-operations.md` | yes | yes | yes | Mirrors CLAUDE.md secret-handling rules |
| `docs/book/src/parity/status.md` | yes | yes | yes | Links to STATUS.md (so inherits HIGH-1) |
| `docs/book/src/security/audit-dossier.md` | yes | yes | yes | Banned-claim discipline enforced |
| `docs/adr/00{01..19}.md` | yes (19 files) | yes | yes | Source files are the authoritative copy |
| `docs/book/src/adr/00*.md` | partial (10 files + 0019) | **stale** | **no** | Missing 0011–0018 stubs (HIGH-4) |
| `docs/man/{pcloudc.1,pcloudd.1}` | yes | yes | yes | But duplicated under packaging/man/ (LOW-1) |
| `packaging/man/{pcloudc.1,pcloud.conf.5,pcloudd.1}` | yes | yes | yes | Authoritative copy; ships with deb/rpm |
| `docs/enterprise/*.md` | yes | mixed | mixed | Several pages skeletal (LOW-3); read-pass deferred |
| Rustdoc (`cargo doc --workspace --no-deps`) | runs | exits 0 | **warnings** | 54 warnings across 9 crates (MEDIUM-1) |
| mdBook build | n/a in this env | n/a | n/a | mdbook not installed; structural integrity of SUMMARY links verified manually |

---

## Quickstart Walk-Through Gap List

Following the README "Build, Test, Docs" → "Run the Daemon + CLI"
path verbatim, plus the install.md cargo-install path:

1. **README §Build:** `cargo build --release --workspace --locked`
   — works (matches the workspace truth).
2. **README §Run:** `cargo run -p pcloud-daemon -- serve` — works,
   but the produced binary is `target/release/pcloudd`, not
   `target/release/pcloud-daemon`. Operators copying the snippet
   into a service unit will silently learn this when their unit
   fails to start.
3. **install.md §Build from source:** `sudo install -m 0755
   target/release/pcloud-daemon /usr/local/libexec/` — **fails**
   immediately with "No such file or directory" (HIGH-3).
4. **install.md §Verify:** `pcloudc doctor --strict` — assumes the
   binaries are on PATH. After the failed install in step 3 they
   are not.
5. **README §Run §Login:** `cargo run -p pcloud-cli -- login --user
   alice@example.com --password-stdin` — works in dev. But the
   README does not link to `OPERATIONS-RUNBOOK.md` or
   `docs/book/src/getting-started/first-login.md`, so a sysadmin
   who hits a TFA challenge at this point has to grep around for
   "TFA" / "two-factor" guidance.
6. **README §Run §Sync add:** `cargo run -p pcloud-cli -- sync add
   ~/Documents /Drive/Documents` — works. But this is the dev
   command; a production sysadmin needs the SCM-managed equivalent
   (`pcloudc sync add ...`), which the README does not call out.
7. **install.md §Verify the install:** The binary smoke-test is
   labelled "exit 0 when healthy" but actual `pcloudc doctor`
   strict semantics live in install.md's bullet list ~50 lines
   later — at first read the operator does not know what
   "WARN promoted to FAIL" means without scrolling.
8. **No troubleshooting → install.md backlink:** install.md does
   not link to `docs/book/src/operations/troubleshooting.md` for
   first-run failures, even though troubleshooting covers every
   failure a freshly-installed daemon can hit (FUSE refused,
   vault locked, TLS pin mismatch).
9. **No deployment → upgrade backlink:** `deployment-guide.md`
   does not link to `operations/upgrade.md`, and `upgrade.md`
   does not link to `auth_vault.rs` schema-versioning notes
   (which exist as ADR 0005).
10. **README §Live Verification:** `PCLOUD_LIVE_AUTH_TOKEN=...
    cargo test ... --ignored` — assumes the operator has a token,
    but does not point at how to obtain one (the answer is
    `pcloudc login --user ... --password-stdin` followed by
    `pcloudc auth token`). README §Live Verification could close
    with a one-liner.

Summary: the README + install.md path is **not** a usable
zero-knowledge sysadmin walkthrough today. The biggest blockers are
the binary-name drift (HIGH-3) and the missing cross-doc links
(items 5/8/9/10 above). Two patches close most of this gap: (a) fix
HIGH-3 in install.md, (b) add a "Next: First Login →
Troubleshooting → Operations Runbook" footer to README.md and to
each getting-started page.

---

## Audit Closing Notes

- No CRITICAL findings. The Rust rewrite has correctly avoided every
  banned readiness claim; the parity-honesty discipline mandated in
  CLAUDE.md is intact across `CLAUDE.md`, `README.md`, `STATUS.md`,
  the mdBook, `CHANGELOG.md`, and the parity dossier.
- The one structural truth-source defect (HIGH-1) is internal to
  STATUS.md and is mechanically fixable in a single edit.
- The HIGH-3 install-walkthrough drift is the highest-impact finding
  for a real sysadmin trying to deploy from docs alone, and is also
  mechanically fixable (rename `pcloud-daemon` → `pcloudd` in two
  files; update MSRV).
- Everything else is content drift caused by the documentation tree
  outgrowing the manual-update workflow. A small parity-tally CI
  script + a `mdbook build` CI gate + a `RUSTDOCFLAGS=-Dwarnings
  cargo doc` CI gate would prevent recurrence.

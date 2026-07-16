# Turn 5 Dimension 1 Audit: Parity / API / CLI / SDK Truth

Read-only review. `pcloud_rev.md` was used as the master prompt. No files were edited by the review agent.

## Findings

### HIGH-1: Partial-row tracker coverage is false

Severity: HIGH

Evidence: `C_FEATURE_PARITY_MATRIX.csv:94`, `:124`, `:138`, `:142`, `:147`, `:148`, and `:168` still contain seven `Partial` rows. `STATUS.md:23` says the gaps are tracked under `gptrev-01 H-01/H-02`, and `STATUS.md:740` says all seven are tracked under `bd-1du.10`. Actual tracker state contradicts that: `.beads/issues.jsonl` has 270 issues and all are `closed`; searches for `gptrev-01`, `CreateFolderPublicLinkWithOptions`, `CreateFolderUpDownLink`, `CreateScreenshotPublicLink`, `CryptoShareFolder`, `CryptoAccountTeamShare`, and `UploadSession` returned zero live tracker hits. The only named crypto live-E2E bead, `.beads/issues.jsonl:148`, is also `closed` while its close reason says live pCloud two-account verification remains gated on operator provisioning.

Remediation: create or reopen concrete live beads for row 94, rows 124/138/142, and rows 147/148/168. Replace historical `bd-1du.*` and non-existent `gptrev-01` references with real issue IDs in CSV, `STATUS.md`, API docs, and the proof checklist.

### HIGH-2: Row 94 SDK `UploadSession` remains non-parity

Severity: HIGH

Evidence: `C_FEATURE_PARITY_MATRIX.csv:94` correctly marks row 94 `Partial`. Code still confirms the blocker: `crates/pcloud-sdk/src/lib.rs:1597` routes `EmbeddedDaemon::start_upload` to `upload_session::run_upload`; `crates/pcloud-sdk/src/upload_session.rs:704` implements that as a legacy one-shot wrapper; `crates/pcloud-sdk/src/upload_session.rs:720` explicitly ignores `request.conflict_mode`; and `crates/pcloud-sdk/src/upload_session.rs:724` calls `daemon.upload_data`, whose public path uses `upload_create` plus `upload_bytes` at `crates/pcloud-sdk/src/lib.rs:1641`. The honest-scope comment at `crates/pcloud-sdk/src/upload_session.rs:36` says the production daemon-backed driver is not joined up.

Remediation: implement a production `UploadSessionDriver` for `EmbeddedDaemon::start_upload`, thread conflict policy to `upload_save`/ifhash semantics, expose pause/resume/cancel over the public route, and add a live pCloud E2E proof before flipping row 94.

### HIGH-3: Crypto share/team-share are still backend-only from user-facing surfaces

Severity: HIGH

Evidence: `C_FEATURE_PARITY_MATRIX.csv:124`, `:138`, and `:142` remain `Partial`. Backend/proto RSA paths exist at `crates/pcloud-backends/src/shares_backend.rs:564` and `:607`, and proto paths exist at `crates/pcloud-proto/src/shares_api.rs:486` and `:527`. But IPC exposes only non-crypto `ShareFolder` and `AccountTeamShare` at `crates/pcloud-ipc/src/methods.rs:557` and `:617`; daemon dispatch routes only those non-crypto variants at `crates/pcloud-daemon/src/runtime.rs:736` and `:768`. No `Request::CryptoShareFolder` or `Request::CryptoAccountTeamShare` exists.

Remediation: add crypto share/team-share IPC variants, daemon orchestration for pubkey/folder-key lookup, CLI and SDK entry points, and real two-account/team live proof. Keep rows `Partial` until those are user-reachable and verified.

### HIGH-4: Public-link specialty helpers remain unreachable from IPC/CLI/SDK

Severity: HIGH

Evidence: `C_FEATURE_PARITY_MATRIX.csv:147`, `:148`, and `:168` remain `Partial`. Proto/backend code exists: `create_folder_public_link_with_options` at `crates/pcloud-proto/src/public_links_api.rs:786` and `crates/pcloud-backends/src/public_link_backend.rs:1000`; `create_folder_updownlink` at `crates/pcloud-proto/src/public_links_api.rs:848` and `crates/pcloud-backends/src/public_link_backend.rs:1047`; `create_screenshot_public_link` at `crates/pcloud-proto/src/public_links_api.rs:820` and `crates/pcloud-backends/src/public_link_backend.rs:1023`. IPC public-link variants stop at normal create/change/tree/upload-link surfaces in `crates/pcloud-ipc/src/methods.rs:450` through `:517`, and daemon dispatch has no specialty variants in `crates/pcloud-daemon/src/runtime.rs:680` through `:717`.

Remediation: add `CreateFolderPublicLinkWithOptions`, `CreateFolderUpDownLink`, and `CreateScreenshotPublicLink` IPC requests, daemon handlers, CLI commands, SDK helpers, and tests.

### MEDIUM-1: Row 149 is reachable, but full root/file live proof is still missing

Severity: MEDIUM

Evidence: `C_FEATURE_PARITY_MATRIX.csv:149` now claims full root/folder/file target support through `CreateTreePublicLinkFromPathTargets`. The code is reachable: IPC variant at `crates/pcloud-ipc/src/methods.rs:1276`, daemon dispatch at `crates/pcloud-daemon/src/runtime.rs:924`, CLI request conversion at `crates/pcloud-cli/src/commands.rs:1448`, and SDK helper at `crates/pcloud-sdk/src/lib.rs:2721`. However, the live E2E test still exercises only the older folder-list alias: `crates/pcloud-live-e2e/tests/tree_link_from_paths.rs:4` documents `CreateTreePublicLinkFromPaths`, `:105` creates folder targets, and `:143` dispatches `Request::CreateTreePublicLinkFromPaths` with two folders only.

Remediation: add a live-gated test for `CreateTreePublicLinkFromPathTargets` using at least one root target, one folder target, and one uploaded file target. Keep current `Implemented` status only if the project accepts parser/proto tests as sufficient proof; otherwise document the live-proof gap.

### MEDIUM-2: Documentation still contradicts current parity truth

Severity: MEDIUM

Evidence: `CLAUDE.md:380` still describes an Audit 05 state with five Partial rows, rows 26/27/93 open, and row 93 as a stub at `CLAUDE.md:391`; current truth is seven Partial rows with row 93 implemented. `CLAUDE.md:434` still calls row 93 Partial. `C_FEATURE_PARITY_REVIEW.md:338` and `:865` say all retained rows are implemented despite the CSV's seven Partial rows. `docs/book/src/faq.md:13` says "three of six" Partial rows and says FUSE lacked a live host run at `:19`. `docs/book/src/security/audit-dossier.md:132` says all retained rows are implemented and `:138` lists already-implemented surfaces as pending/missing. `docs/book/src/parity/status.md:104` through `:128` retain stale footnotes claiming daemon mount wiring and row 187 remain Partial, while `C_FEATURE_PARITY_MATRIX.csv:187` is `Implemented`.

Remediation: regenerate all count/status-bearing docs from `STATUS.md` and `C_FEATURE_PARITY_MATRIX.csv`. Fence historical sections clearly as superseded, or remove current-tense claims that conflict with the CSV.

### LOW-1: `STATUS.md` has malformed headline Markdown

Severity: LOW

Evidence: `STATUS.md:27` opens inline code for `149 / 7 / 0 / 30 (186 rows)` but does not close the backtick before the bold marker.

Remediation: close the inline-code span so rendered docs do not hide or mangle the authoritative headline.

## Positive Checks

Row 93's Turn 4 fix appears wired: IPC carries separate `offset` and `source_offset` at `crates/pcloud-ipc/src/methods.rs:1229`; daemon passes both to `upload_write_from_file` at `crates/pcloud-daemon/src/runtime.rs:3603`; CLI parses distinct offsets at `crates/pcloud-cli/src/app.rs:2847`; SDK sends both at `crates/pcloud-sdk/src/lib.rs:1697`.

Rejected-rationale coverage is clean: the CSV has 30 `Rejected` rows and `REJECTED-RATIONALES-14042026.md` has matching `Row N` sections with no missing or orphan rationales.

## Commands / Tests Run

- `git status --short`: dirty tree present before review; no edits made.
- CSV/rationale/tracker Python check: `186` rows; `149 Implemented / 7 Partial / 0 Missing / 30 Rejected`; missing rationales `[]`; orphan rationales `[]`; beads `270 closed`.
- `cargo test -p pcloud-cli upload_write_from_file_parses_distinct_offsets`: passed.
- `cargo test -p pcloud-cli create_tree_link_from_paths_accepts_root_folders_and_files`: passed.
- `cargo test -p pcloud-proto tree_public_link_from_paths_resolves_ids_and_sends_expected_params`: passed.
- `cargo test -p pcloud-sdk upload_write_from_file_requires_authentication`: passed.
- `cargo test -p pcloud-sdk tree_public_link_from_targets_rejects_empty_targets`: passed.
- `cargo test -p pcloud-sdk start_upload_round_trip_completes_on_development_transport`: passed.
- SDK public-doc sweep attempt failed because `/tmp` was full. Live E2E tests were not run; no pCloud credentials/env gate were provided.

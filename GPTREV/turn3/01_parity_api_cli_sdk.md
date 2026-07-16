# Turn 3 Subagent 01 Report: Parity / API / CLI / SDK

No files modified. I did not write `AUDIT_REPORT.md`.

## Findings

### HIGH: Row 93 `upload_writefromfile` is still not fully C-shaped through IPC/CLI

Severity: HIGH

Evidence: `UploadWriteFromFileRequest` models both destination `upload_offset` and source `source_offset` and serializes them as separate `uploadoffset` and `offset` params in `crates/pcloud-proto/src/methods/upload.rs:289`, `crates/pcloud-proto/src/methods/upload.rs:298`, `crates/pcloud-proto/src/methods/upload.rs:331`, `crates/pcloud-proto/src/methods/upload.rs:336`. The IPC variant exposes only one `offset` in `crates/pcloud-ipc/src/methods.rs:1241`, and daemon dispatch passes that same value as both destination and source offset in `crates/pcloud-daemon/src/runtime.rs:3577`. CLI parsing likewise accepts only one `<OFFSET>` in `crates/pcloud-cli/src/app.rs:2819`.

Impact: The backend can encode the C primitive, but daemon/CLI callers cannot express the full C call shape. Any non-aligned server-side copy, resume, or splice requiring different source and destination offsets is impossible or silently wrong.

Remediation: Add separate `upload_offset` and `source_offset` fields to `Request::UploadWriteFromFile`, CLI parser/help/completion, SDK helper if exposed, IPC roundtrip tests, and daemon dispatch. Preserve old one-offset input only as an explicit compatibility alias if needed.

### HIGH: Public-link specialty parity rows remain backend-only

Severity: HIGH

Evidence: Rows 147, 148, and 168 are `Partial` in `C_FEATURE_PARITY_MATRIX.csv:147`, `C_FEATURE_PARITY_MATRIX.csv:148`, and `C_FEATURE_PARITY_MATRIX.csv:168`. Backend/proto functions exist at `crates/pcloud-backends/src/public_link_backend.rs:1000`, `crates/pcloud-backends/src/public_link_backend.rs:1023`, and `crates/pcloud-backends/src/public_link_backend.rs:1047`; proto mirrors them at `crates/pcloud-proto/src/public_links_api.rs:786`, `crates/pcloud-proto/src/public_links_api.rs:820`, and `crates/pcloud-proto/src/public_links_api.rs:848`. IPC/daemon only expose basic `CreateFilePublicLink` / `CreateFolderPublicLink` in `crates/pcloud-ipc/src/methods.rs:450` and `crates/pcloud-ipc/src/methods.rs:455`, with runtime routing only for those basic variants in `crates/pcloud-daemon/src/runtime.rs:650`.

Impact: `psync_folder_public_link_full`, `psync_folder_updownlink_link`, and `psync_screenshot_public_link` are not reachable by CLI, SDK, or daemon IPC callers, so enterprise users cannot exercise retained C public-link workflows despite backend code existing.

Remediation: Add `Request::CreateFolderPublicLinkWithOptions`, `Request::CreateFolderUpDownLink`, and `Request::CreateScreenshotPublicLink`; wire daemon handlers, CLI commands, completions, SDK typed helpers, IPC proptests, and live/dev transport tests.

### HIGH: Crypto share/team-share backend exists but no IPC/CLI/SDK route reaches it

Severity: HIGH

Evidence: Matrix rows 124, 138, and 142 are `Partial` at `C_FEATURE_PARITY_MATRIX.csv:124`, `C_FEATURE_PARITY_MATRIX.csv:138`, and `C_FEATURE_PARITY_MATRIX.csv:142`. Backend functions exist in `crates/pcloud-backends/src/shares_backend.rs:564` and `crates/pcloud-backends/src/shares_backend.rs:607`. IPC only has non-crypto `ShareFolder` and `AccountTeamShare` variants in `crates/pcloud-ipc/src/methods.rs:557` and `crates/pcloud-ipc/src/methods.rs:617`; runtime routes those to non-crypto handlers in `crates/pcloud-daemon/src/runtime.rs:711` and `crates/pcloud-daemon/src/runtime.rs:743`.

Impact: Crypto-aware folder sharing and crypto team sharing cannot be performed through daemon IPC, CLI, or SDK. The row 124 note frames the remaining gate as live E2E, but row 138 shows an actual reachability gap for the same C symbol.

Remediation: Add explicit crypto share IPC variants and daemon orchestration that fetches recipient/team public keys, validates unlocked crypto state, prompts/handles temppass securely, calls the RSA backend path, and exposes CLI/SDK typed methods. Update row 124 notes to include the IPC gap, or merge/deduplicate rows 124 and 138.

### HIGH: Parity truth surfaces disagree on current counts and row 93 status

Severity: HIGH

Evidence: Parsing `C_FEATURE_PARITY_MATRIX.csv` produced `149 Implemented / 7 Partial / 30 Rejected`, while `STATUS.md` claims `150 / 6 / 0 / 30` at `STATUS.md:31` and `STATUS.md:656`. Row 93 is still `Partial` in `C_FEATURE_PARITY_MATRIX.csv:93`, but `STATUS.md:23` and `API-REFERENCE.md:59` claim it is implemented. `C_FEATURE_PARITY_REVIEW.md:12` still states `156 / 2 / 0 / 28`, and `C_FEATURE_PARITY_REVIEW.md:310` says all retained rows are implemented.

Impact: The lead parity gate can be closed against contradictory evidence. Auditors, release notes, and SDK/API consumers cannot tell whether CSV, STATUS, or API reference is authoritative.

Remediation: Choose CSV-derived counts as the generated source, update `STATUS.md`, `API-REFERENCE.md`, and `C_FEATURE_PARITY_REVIEW.md`, and add a CI/doc test that parses the CSV and fails when published counts or "all retained rows implemented" claims drift.

### MEDIUM: CLI help and completion are inconsistent with the actual parser

Severity: MEDIUM

Evidence: Help advertises `create-file-link <FILEID>` and `create-folder-link <FOLDERID>` in `crates/pcloud-cli/src/app.rs:304`, but IPC and parser use absolute remote paths at `crates/pcloud-ipc/src/methods.rs:450` and `crates/pcloud-cli/src/app.rs:1825`. Completion advertises `upload write-from-file` as a local-file operation with only `upload-id` and `local-path` args at `crates/pcloud-cli/src/completion.rs:437`, while the parser expects `<UPLOAD_ID> <SOURCE_FILEID> <SOURCE_HASH> <OFFSET> <COUNT>` at `crates/pcloud-cli/src/app.rs:2819`.

Impact: Operators following `--help` or shell completion can pass numeric IDs where paths are required, or a local path where server-side copy IDs/hash are required. That turns parity-valid commands into avoidable usage failures.

Remediation: Generate help and completion from one declarative command spec, or add tests asserting help/completion arg names match `parse_inputs_for_command`. Fix public-link help to say path, and fix upload completion to expose the five numeric server-side-copy args.

### MEDIUM: SDK breadth is not parity-complete without raw IPC exposure

Severity: MEDIUM

Evidence: SDK publicly exposes raw `pcloud_ipc::Request` / `Response` as part of `EmbeddedDaemon::dispatch` in `crates/pcloud-sdk/src/lib.rs:63` and `crates/pcloud-sdk/src/lib.rs:1369`. Searches found no typed SDK helpers for public-link CRUD or share/team operations; the typed public-link SDK surface visible in this pass is `send_publink` at `crates/pcloud-sdk/src/lib.rs:2530` and `create_tree_public_link_from_paths` at `crates/pcloud-sdk/src/lib.rs:2605`.

Impact: Embedders must bind directly to internal IPC/model crates for large parts of the parity surface. That weakens semver isolation and makes the SDK less enterprise-ready despite the presence of daemon/CLI paths.

Remediation: Add typed SDK DTOs and helpers for public-link CRUD, upload-link CRUD, share request/list/mutate, account team share, crypto-share once reachable, and full-shape `upload_writefromfile`. Mark raw `dispatch` as low-level/unstable or put it behind an explicit feature.

### MEDIUM: `STATUS.md` still contains stale non-parity engineering state for backup sync flavor

Severity: MEDIUM

Evidence: `STATUS.md:686` says `backup`, `upload-only`, `up`, and `local-to-remote` all map to `SyncType::UploadOnly`. The parser maps `backup|backup-archive|archive|keep-remote` to `SyncType::BackupArchive` in `crates/pcloud-cli/src/app.rs:3197`, and the model defines `BackupArchive` as deletion-safe in `crates/pcloud-model/src/sync.rs:172`.

Impact: Enterprise readiness docs misstate current CLI behavior and can lead reviewers to chase already-landed work or distrust the status file.

Remediation: Refresh the open engineering bead section in `STATUS.md` or move historical stale text under a superseded heading.

## Checks Run

```text
sed -n '1,220p' pcloud_rev.md
python3 csv parse of C_FEATURE_PARITY_MATRIX.csv
rg/nl/sed targeted reads across STATUS.md, C_FEATURE_PARITY_REVIEW.md, API-REFERENCE.md, REJECTED-RATIONALES-14042026.md
rg targeted reachability searches for upload_writefromfile, public-link specialty rows, crypto share rows, SDK helpers, CLI help/completion
cargo check -p pcloud-sdk --examples
cargo test -p pcloud-cli -- --nocapture
cargo test -p pcloud-backends network_upload_write_from_file -- --nocapture
```

Verification results: SDK examples compile; full `pcloud-cli` tests pass `274 passed / 0 failed`; targeted `pcloud-backends` upload-write-from-file tests pass `2 passed / 0 failed`.

## Limitations

No live pCloud account or hardware verification was performed. I did not inspect `.beads/`, `GPTREV/`, `CLAUDEREV/`, `target/`, or generated tracker output as requested. I did not run full workspace tests.

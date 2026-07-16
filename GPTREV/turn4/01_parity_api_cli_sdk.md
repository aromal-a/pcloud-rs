# Turn 4 Dimension 1 Audit: Parity/API/CLI/SDK Truth

Read-only audit using `pcloud_rev.md` as the master prompt. No files were edited.

## Findings

### HIGH-1: SDK UploadSession row is falsely marked Implemented

Severity: HIGH

Evidence: `C_FEATURE_PARITY_MATRIX.csv:94` claims SDK `UploadSession` is Implemented with daemon-backed chunking, conflict modes, and a `DaemonSessionDriver`. Current code contradicts this: `crates/pcloud-sdk/src/upload_session.rs:44-46` says the row stays Partial until live E2E, `crates/pcloud-sdk/src/upload_session.rs:89-96` says `ConflictMode` is not threaded to wire, and `crates/pcloud-sdk/src/upload_session.rs:719` discards it. `crates/pcloud-sdk/src/lib.rs:1596-1597` routes public `start_upload` to `run_upload`, which uses single-shot `upload_data` at `crates/pcloud-sdk/src/lib.rs:1640-1653`, not the chunked driver. Search found no `DaemonSessionDriver` or `EmbeddedDaemon::start_chunked_upload`.

Remediation: either flip row 94 back to Partial or implement a production `UploadSessionDriver`, expose a real SDK entry point, thread conflict policy into `upload_save`, and add live/daemon-backed tests.

### HIGH-2: Row 93 `upload_writefromfile` docs are stale, and IPC still loses C semantics

Severity: HIGH

Evidence: `STATUS.md:15`, `STATUS.md:704`, `API-REFERENCE.md:63-75`, and `C_FEATURE_PARITY_MATRIX.csv:93` say the daemon returns a stub and CLI was removed. Current code wires the path: daemon dispatch at `crates/pcloud-daemon/src/runtime.rs:879-891`, handler at `crates/pcloud-daemon/src/runtime.rs:3523-3586`, backend execution at `crates/pcloud-backends/src/transfer_backend.rs:761-803`, CLI parsing at `crates/pcloud-cli/src/app.rs:871-872` and `crates/pcloud-cli/src/app.rs:2820-2854`, and command wiring at `crates/pcloud-cli/src/commands.rs:1317-1323`.

However, IPC/CLI still model only one `offset`: `crates/pcloud-ipc/src/methods.rs:1226-1247` and `crates/pcloud-cli/src/app.rs:2821-2847`. The proto request has distinct `upload_offset` and `source_offset` at `crates/pcloud-proto/src/methods/upload.rs:289-299` and `crates/pcloud-proto/src/methods/upload.rs:331-337`, but runtime passes the same offset for both at `crates/pcloud-daemon/src/runtime.rs:3569-3575`. The proptest comment repeats the reduced shape at `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:623-640`.

Remediation: add `source_offset` to IPC/CLI/SDK request shape, preserve backward compatibility if needed, update roundtrip tests, then update STATUS/API/CSV to describe the real remaining gap.

### HIGH-3: Row 149 tree public link is marked Implemented but CLI/daemon/SDK support folder paths only

Severity: HIGH

Evidence: `C_FEATURE_PARITY_MATRIX.csv:149` marks `ptree_public_link` Implemented. The proto layer supports root/folders/files at `crates/pcloud-proto/src/public_links_api.rs:56-69`, `crates/pcloud-proto/src/public_links_api.rs:898-900`, and `crates/pcloud-proto/src/public_links_api.rs:949-962`. IPC exposes only `paths: Vec<String>` with no file/root distinction at `crates/pcloud-ipc/src/methods.rs:1258-1265`, daemon resolves every path as a folder and sends `file_ids_csv: None` at `crates/pcloud-daemon/src/runtime.rs:3630-3654`, and SDK documents this limitation at `crates/pcloud-sdk/src/lib.rs:2564-2571` while its example includes a file path at `crates/pcloud-sdk/src/lib.rs:2599-2602`. The live E2E test creates two folders only at `crates/pcloud-live-e2e/tests/tree_link_from_paths.rs:105-146`.

Remediation: change IPC/CLI/SDK to accept structured `TreePublicLinkPaths { root, folders, files }`, route file paths through `resolve_file`, add live coverage for at least one file path, or downgrade row 149 to Partial.

### HIGH-4: Public-link specialty rows remain backend-only and one API table contradicts itself

Severity: HIGH

Evidence: rows 147/148/168 are Partial in `C_FEATURE_PARITY_MATRIX.csv:147-148` and `C_FEATURE_PARITY_MATRIX.csv:168`. Proto/backend functions exist at `crates/pcloud-proto/src/public_links_api.rs:786`, `crates/pcloud-proto/src/public_links_api.rs:820`, `crates/pcloud-proto/src/public_links_api.rs:848`, `crates/pcloud-backends/src/public_link_backend.rs:1000`, `crates/pcloud-backends/src/public_link_backend.rs:1023`, and `crates/pcloud-backends/src/public_link_backend.rs:1047`, but IPC request variants stop at normal file/folder/tree/upload-link operations at `crates/pcloud-ipc/src/methods.rs:450-517`. `API-REFERENCE.md:87` incorrectly lists folder link create with `_with_options` as Implemented, while `API-REFERENCE.md:97-99` lists the same specialty surfaces as Partial.

Remediation: add `CreateFolderPublicLinkWithOptions`, `CreateFolderUpDownLink`, and `CreateScreenshotPublicLink` IPC variants, daemon dispatch, CLI commands, SDK helpers, and tests; fix the API table.

### HIGH-5: Crypto share/team-share parity is still unreachable from IPC/CLI/SDK

Severity: HIGH

Evidence: rows 124/138/142 remain Partial at `C_FEATURE_PARITY_MATRIX.csv:124`, `C_FEATURE_PARITY_MATRIX.csv:138`, and `C_FEATURE_PARITY_MATRIX.csv:142`. Backend/proto code exists at `crates/pcloud-backends/src/shares_backend.rs:484`, `crates/pcloud-backends/src/shares_backend.rs:564`, `crates/pcloud-backends/src/shares_backend.rs:607`, `crates/pcloud-proto/src/shares_api.rs:486`, and `crates/pcloud-proto/src/shares_api.rs:527`, but IPC exposes only non-crypto `ShareFolder` and `AccountTeamShare` at `crates/pcloud-ipc/src/methods.rs:557-631`, and daemon dispatch routes only those non-crypto variants at `crates/pcloud-daemon/src/runtime.rs:711-750`. The RSA test itself states live two-account verification remains operator work at `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs:21-23`.

Remediation: add crypto share/team-share IPC variants, daemon orchestration for pubkey/folder-key lookup, CLI/SDK entry points, and a live two-account E2E gate before flipping rows.

### MEDIUM-1: Parity documentation is internally inconsistent and stale

Severity: MEDIUM

Evidence: `C_FEATURE_PARITY_REVIEW.md:310` says all retained rows are Implemented, contradicting the CSV/STATUS seven Partial rows. `PARITY-PROOF-CHECKLIST.md:12-13`, `PARITY-PROOF-CHECKLIST.md:30-31`, and `PARITY-PROOF-CHECKLIST.md:83-89` still cite 28 rejected rows and 156/2/0/28 counts, while current CSV parse is 149/7/0/30. `docs/book/src/parity/status.md:62-69` still says two Partial rows and row 149 is open. `CLAUDE.md:53-57` and `CLAUDE.md:80-83` say old `bd-1du.*` IDs are historical/no live beads, but `STATUS.md:741` says all seven Partial rows are tracked under `bd-1du.10`.

Remediation: make `STATUS.md` and CSV the only count-bearing sources, regenerate all parity/checklist/book summaries from them, and replace historical bead IDs with current live tracker IDs or explicit no-live-bead status.

## Positive Checks

Rejected-rationale coverage is clean: CSV has 30 Rejected rows and every one has a matching `Row N` rationale; no orphan rationales were found.

## Commands/Tests Run

- `python3` CSV parse: `186` rows, `149 Implemented / 7 Partial / 0 Missing / 30 Rejected`.
- `python3` rejected-rationale cross-check: `missing rationale []`, `orphan rationale rows []`.
- `cargo test -p pcloud-backends network_upload_write_from_file_drives_server_side_copy`: passed, 1 test.
- `cargo test -p pcloud-sdk --test upload_session_chunked`: passed, 4 tests.
- `cargo test -p pcloud-sdk upload_session`: passed, 4 matching lib tests; integration tests filtered out.
- `cargo test -p pcloud-cli upload_write_from_file`: compiled; 0 matching tests.
- `cargo check -p pcloud-sdk --examples`: passed; warning only about vendored password dictionary.
- `cargo test -p pcloud-proto tree_public_link_from_paths_resolves_ids_and_sends_expected_params`: passed, 1 test.

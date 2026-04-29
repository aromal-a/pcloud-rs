# GPTREV 07 — CLI Parser / SDK Public Surface Fix Report

Date: 2026-04-26
Stream: G7
Status: All actionable findings fixed; compile clean; 274 tests pass; all examples build.

## Findings Addressed

### 07-H-01 (HIGH, FIXED): `stat` off-by-one — command name sent as path

- **File:** `crates/pcloud-cli/src/app.rs` — `Command::Stat` arm in `parse_inputs_for_command`
- **Fix:** Changed `args.get(1)` → `args.get(2)`. Index 0 is the binary name, index 1 is "stat", index 2 is the user-supplied path.
- **Regression tests added:** `stat_parses_path_from_correct_position`, `stat_without_path_is_an_error`

### 07-H-02 (HIGH, FIXED): `change-link-password` and `crypto change-password` bypass `--allow-argv-password` gate

- **Files:** `crates/pcloud-cli/src/app.rs` — `Command::ChangeLinkPassword`, `Command::CryptoChangePassword`, `Command::CryptoChangePasswordUnlocked` arms
- **Fix:** Added `--allow-argv-password` guard matching the pattern used by `SubmitAuthToken`, `SubmitTwoFactorCode`, and `SubmitCryptoPassword`. `std::process::exit(2)` on ungated argv secrets. Updated `allowed_flags_for` to include `--allow-argv-password` for both `ChangeLinkPassword` and `CryptoChangePassword*`.
- **Existing test updated:** `change_link_password_input_parses_explicit_values` — added `--allow-argv-password` to the test args.

### 07-H-04 (HIGH, FIXED): `delete_file` and `rename_file` were stubs

- **File:** `crates/pcloud-sdk/src/lib.rs`
- **Fix:** Wired `delete_file` to dispatch `Request::FileDeleteByPath` and `rename_file` to dispatch `Request::RenamePath`. Both existed in `pcloud-ipc` and were handled by the daemon runtime. Added absolute-path validation before dispatch.
- Also added `AuthHelperError::Login` variant to support the `login` / `login_with_token` helpers introduced by a concurrent stream.

### 07-M-01 (MEDIUM, FIXED): Completion tree drift

- **File:** `crates/pcloud-cli/src/completion.rs`
- **Fixes:**
  - `sync change-type`: was `<local-path> <two-way|upload-only|download-only>` — fixed to `<sync-id: u64> <bilateral|full|mirror|download-only|upload-only|backup>` matching the runtime parser.
  - `sync suggest`: `--limit` renamed to `--max` matching the runtime flag name.
  - `verify`: added `--yes` flag (parser allows it via `allowed_flags_for`).
  - `upload write-from-file`: added to the `upload` subcommand group (was absent, parser supported it).

### 07-M-02 (MEDIUM, FIXED): `parse_flag_string` ignored `--flag=value` inline form

- **File:** `crates/pcloud-cli/src/app.rs` — `parse_flag_string` function
- **Fix:** Added `strip_prefix(&format!("{flag}="))` branch matching the existing `parse_flag_i64` implementation.
- **Regression tests added:** `sync_suggest_max_accepts_inline_equals_form`, `sync_suggest_max_accepts_spaced_form`

### 07-M-03 (MEDIUM, FIXED): `create_tree_public_link_from_paths` advertised mixed file/folder but only handled folders

- **Files:** `crates/pcloud-sdk/src/lib.rs`, `crates/pcloud-cli/src/app.rs`
- **Fix:** Updated doc on `create_tree_public_link_from_paths` to add a `# Limitation` section explicitly stating that all paths are treated as folder paths and file path resolution is not implemented. Updated CLI help text for `create-tree-link` and `create-tree-link-from-paths` to say "folder ids/paths only" instead of "mixed files".

### 07-M-04 (MEDIUM, DOC FIXED): `ConflictMode` accepted but silently discarded

- **File:** `crates/pcloud-sdk/src/upload_session.rs`
- **Fix:** Added `# Current Status` section to `ConflictMode` rustdoc explicitly documenting that the value is NOT threaded through to the wire layer and callers must not rely on it in production code.

### 07-M-06 (MEDIUM, DOC FIXED): SDK semver docs inaccurate — claimed `ConfigProfile`/`Environment` were not re-exported

- **File:** `crates/pcloud-sdk/src/lib.rs` module-level docstring
- **Fix:** Replaced the misleading claim with an accurate enumeration of which workspace-internal types appear in public signatures (ConfigProfile, Environment, Request/Response, plugin API types, CreatedTreePublicLink) and why. Added `# TLS Backend` section documenting the rustls-only limitation.

## Findings Not Fixed (Assessment)

### 07-H-03: SDK semver — raw workspace types in public signatures

Not fixed as a code change. The types (`ConfigProfile`, `Environment`, `Request`/`Response`, plugin API) are structurally required by the raw-dispatch and plugin registration APIs. Wrapping them in SDK-owned newtypes would be a large API break. The semver documentation has been corrected to honestly describe what is exported. Full newtype wrapping is tracked as a future parity work item under `bd-1du.10`.

### 07-M-05: README drift

Not fixed. README content is documentation only and no code correctness issue. The READMEs should be updated in a separate docs-only PR by a human reviewer.

### 07-M-07: IPC coverage matrix

Not fixed as code. This is a documentation/tooling gap; a coverage matrix generator would require significant new tooling. Flagged for follow-up under the parity proof phase.

## Verification

```
cargo check -p pcloud-cli -p pcloud-sdk --all-targets  → CLEAN
cargo test -p pcloud-cli                                → 274 passed / 0 failed
cargo test -p pcloud-sdk --tests                       → 53 passed / 0 failed
cargo build --examples -p pcloud-sdk                   → CLEAN
```

## Files Modified

- `crates/pcloud-cli/src/app.rs` — stat off-by-one fix, argv-password gates, `parse_flag_string` inline=value, help text, regression tests
- `crates/pcloud-cli/src/completion.rs` — sync change-type positionals, suggest --max, verify --yes, upload write-from-file
- `crates/pcloud-sdk/src/lib.rs` — delete_file / rename_file wired via IPC dispatch, semver doc correction, tree link doc limitation, AuthHelperError::Login variant
- `crates/pcloud-sdk/src/upload_session.rs` — ConflictMode limitation documentation

# Section 8: CLI & SDK Surface
## Date: 2026-04-17
## Findings

### CRITICAL [0]
_None._

### HIGH [3]
- H1. `submit-password <user> <pw>` / `auth <pw>` accept password as positional argv → leaks via `/proc/<pid>/cmdline` and shell history (stderr warning only; no hard refusal or TTY gate).
- H2. SDK `[features]` section is entirely absent in `crates/pcloud-sdk/Cargo.toml`; no `tls-rustls` / `tls-native` / `default` declaration. Consumers cannot select a TLS backend, and the section-8 feature-flag matrix requirement is unmet.
- H3. SDK surface missing wrappers for core `transfer_api.rs` operations: `rename_file`, `delete_file`, `upload_info`, `upload_delete`, `upload_blockchecksums_begin`, `get_checksum_link` — only uploads + `get_file_link` + `download_file` are exposed. Callers must reach into `pcloud_proto` to delete/rename, defeating SDK encapsulation.

### MEDIUM [6]
- M1. IPC methods with no CLI path: `Method::Health` (`healthz` maps to `HealthDetailed`, OK), `Method::SetAuthPersistence` via `AuthSave` (OK), **but** `Method::GetCryptoStatus` separately, `Request::SyncRootChangeType` is reachable but `Request::MountForceUnmount` CLI surface exists only as an app.rs-side `--force-umount` flag, not a listed subcommand in `completion.rs`. Audit invariant "every IPC method reachable → CLI surface OR daemon-internal tag" is not documented.
- M2. CLI help text (`app.rs` `help_text`) is a 442-line static string *duplicating* what `completion.rs::build_cli` declares. These two surfaces can drift; no test ties them together.
- M3. `completion.rs` contains two `clap::Command` trees but **no generator invocation pinning output files** — bash/zsh/fish/powershell scripts are emitted on demand via `pcloudc completion <shell>`, never stored/shipped. Packagers must run the binary to populate `/etc/bash_completion.d/pcloudc`. No Cargo build step ships pre-generated completions.
- M4. `crates/pcloud-sdk/examples/` contains only **one** example (`login_and_list.rs`). No example demonstrates `upload_file` / `upload_data` / `download_file` / public-link creation, contrary to audit requirement "cover the main operations (upload, download, auth)".
- M5. SDK `lib.rs` has `#![deny(missing_docs)]` (good) but several pub items in `upload_session.rs` module lack per-variant error doc (`UploadError::Create`, `UploadError::Write`, etc. spot-checked at lines 198–232).
- M6. No SDK test asserts a `download_file` happy path on *non-development* transport; test at `lib.rs:3769` uses `Environment::Development`. Live-server parity is not exercised at SDK layer.

### LOW [7]
- L1. `ExitCode` enum is well-documented & tested (`exit_code.rs:144-211`) but the `classify_transport_error` substring matcher is fragile — unknown transport errors become `GenericError` (exit 1) rather than `Network` (exit 4). Substring-match order matters and is undertested.
- L2. `version_banner()` reports `GIT_HASH` + `BUILD_PROFILE`, falls back to `"unknown"` when `.git` absent. Good design, but workspace version is printed without a "dirty tree" flag (e.g. no `git describe --dirty`).
- L3. Completion test `build_cli_subcommand_count_at_least_80` (`completion.rs:532`) asserts ≥80 subcommands at the top level of the completion tree only, not the full Command enum count — passes today (visual count ≈90 top-level stubs) but does not detect deletions of grouped subcommands.
- L4. `crates/pcloud-cli/tests/` has just 2 integration files (`field_selector_cli.rs`, `migrate_fixture.rs`); no end-to-end subcommand dispatch test.
- L5. SDK re-exports `pcloud_proto::Notification` (`lib.rs:105`) — leaks an implementation-crate type into the stable SDK surface. Semver-risky: `pcloud-proto` is a private crate (not `[publish = false]`, but semver-coupled).
- L6. `rpassword` (`Cargo.toml:22`) is pinned at `7.3` (not workspace-managed); may drift from other crates.
- L7. The interactive-password `read_masked` fallback for non-Linux falls through to `rpassword::read_password()` silently (`prompt.rs:248-251`) — no user-visible notice that masked-echo UX is unavailable on non-Linux.

---

## Detailed Findings

### 8.1 CLI — Subcommand completeness

`crates/pcloud-cli/src/commands.rs:35-608` defines `Command` enum with ~120 variants (counted: `Help`, `Status`, `Health`, `Pending`, `Slo`, `ListLinks`, `ListUploadLinks`, `ShowLink`, `DeleteLink`, `CreateFileLink`, `CreateFolderLink`, `ChangeLinkExpire`, `ChangeLinkPassword`, `ChangeLinkUpload`, `CreateUploadLink`, `DeleteUploadLink`, `CreateTreeLink`, `ListLinkAccess`, `AddLinkAccess`, `RemoveLinkAccess`, `ListBookmarks`, `RemoveBookmark`, `ChangeBookmark`, `SyncList`, `SyncAdd`, `SyncRemove`, `SyncStatus`, `SyncChangeType`, `UserInfo`, `Pause`, `Resume`, `LoginBegin`, `Logout`, `SendTwoFactorSms`, `SendTwoFactorNotification`, `SubmitPassword`, `SubmitAuthToken`, `SubmitTwoFactorCode`, `SubmitRecoveryCode`, `SubmitCryptoPassword`, `AuthSave`, `LockCrypto`, `Shutdown`, `Drain`, `Start`, `ListIncomingShares` … `CryptoHint`, `SyncSuggest`, `SyncIsSyncable`, `AccountVerifyEmail` … `ValueHas`, `HealthDetailed`).

`crates/pcloud-ipc/src/methods.rs:37-216` — `Method` enum: 44 variants (Linux-inspected). `Request` enum (`:260-1022`): ≈60 argument-bearing variants.

Cross-reference: every `Method` variant is reachable from `Command::into_request` paths verified by grepping `=> Command::` and `Method::`. No orphan `Method` variants detected. However there is **no test** asserting that coverage; M1 above.

### 8.2 Help accuracy — clap argument shapes

`app.rs` uses a hand-written lexer (`parse_command` at line 1315+) rather than clap derive. clap is only used to build the `completion.rs` tree for shell tab-completion. Consequence: help text (`app.rs:16-442`) and the completion tree (`completion.rs:35-459`) can desynchronize from the actual parser (M2).

Spot-checked five subcommands mapping to Request variants:
- `sync add <LOCAL> <REMOTE> [--type FLAVOR]` → `Request::SyncRootAdd { local_path, remote_path, sync_type }` ✓ fields match.
- `publink change-password <ID> [PW]` → `Request::ChangePublicLinkPassword { link_id, password: Option<RedactedString> }` ✓ matches.
- `upload create <LOCAL> <REMOTE_NAME> [--parent N] [--total-bytes N] [--conflict ...]` → `Request::UploadCreate { local_path, remote_name, parent_folder_id, total_bytes, conflict_mode }` ✓ matches.
- `backup-snapshot create ... --zstd-level N` → `Request::BackupSnapshot { zstd_level, ... }` ✓ matches.
- `audit verify [--from ID] [--to ID]` → `Request::AuditVerifyChain { range: AuditVerifyRange { from, to } }` ✓ matches.

No shape mismatches found in the spot check.

### 8.3 Error exit codes

`crates/pcloud-cli/src/exit_code.rs:57-87` — `ExitCode { Ok=0 .. Internal=8 }` enum, public-contract-stable per doc, documented in `--help` via `EXIT_CODE_HELP` constant. Tests at `exit_code.rs:144-211` cover mapping. No places always return exit 0 regardless of outcome detected in `main.rs` skim. Good discipline overall — L1 is the only quibble.

### 8.4 Secret masking (interactive prompts)

`crates/pcloud-cli/src/prompt.rs`:
- `read_secret` (L99-110) uses `rpassword::read_password()` — no echo. ✓
- `read_masked` (L130-143) for 2FA: non-canon + no-echo termios, RAII restore via `Restore { ... } Drop` (L185-193) → shell always restored, including on panic. ✓
- `read_line` for usernames (echo on) — expected behaviour.
- `SecretString` wrapping happens at caller boundaries; prompts return `String`, the CLI immediately wraps.

Prompt hygiene is good. One cosmetic issue: non-Linux platforms silently degrade (`prompt.rs:248-251`, L7).

### 8.5 Shell completion

`crates/pcloud-cli/src/completion.rs`:
- `build_cli()` (L35) builds a parallel clap tree.
- `generate_completion()` (L462) uses `clap_complete::generate`.
- `parse_shell` (L470-479) accepts bash/zsh/fish/elvish/powershell/pwsh.
- Tests (L491-583) assert non-empty output for every shell and a minimum subcommand count.

No pre-generated shell-completion artifacts are committed to the repo or emitted at build time (M3). Debian/RPM packagers must invoke the binary at install time.

### 8.6 Version reporting

`crates/pcloud-cli/src/main.rs:56-66` — `version_banner()` = `"pcloudc <CARGO_PKG_VERSION> (<GIT_HASH>, <BUILD_PROFILE>)"`. Both optional env vars; falls back to `"unknown"`.

`crates/pcloud-cli/build.rs` — build script injects `GIT_HASH` (short SHA) and `BUILD_PROFILE`. Emits `cargo:rerun-if-changed=.git/HEAD` so branch switch triggers rebuild. Soft-fails when git is absent. ✓ Meets the "workspace version + git SHA" requirement. L2 is a nice-to-have.

### 8.7 Secret exposure (argv)

**H1.** `submit-password [USER] [PW]` and `auth <PW>` both accept cleartext on argv (`app.rs:3009-3026`). A stderr warning is printed but the CLI still dispatches. The audit recommendation is to either (a) refuse non-stdin password on argv in non-interactive mode, or (b) at minimum exit non-zero without `--i-accept-cmdline-password` acknowledgement.

Mitigating controls present: `--password-stdin`, `--password-env <VAR>` (auto-unsets env after read), interactive prompt fallback.

### 8.8 Subcommand count test

`completion.rs:531-539` — `build_cli_subcommand_count_at_least_80` asserts ≥80 top-level subcommands in the completion tree. Today the tree visibly contains ≈90 top-level `.subcommand(...)` calls; test passes (L3).

---

### 8.9 SDK — API surface semver discipline

`crates/pcloud-sdk/src/lib.rs`:
- `pub use pcloud_proto::Notification;` (L105) — re-exports an internal-crate type (L5, LOW).
- `pub use upload_session::{...}` (L97-100) — these are the SDK's own module items, fine.
- `EmbeddedDaemon`, `EmbeddedDaemonBuilder` and the full helper surface are owned by the SDK. ✓

No other cross-crate pub re-exports found.

### 8.10 SDK — Doc coverage

`#![deny(missing_docs)]` at `lib.rs:67` enforces doc presence at the module level. Module-level docstring (L1-65) covers conventions. However inside `upload_session.rs`:
- `UploadError` enum variants (`upload_session.rs:198-231`) — variant-level docs are present on most but per-cause wrapping is terse.

SDK surface enums `SdkError`, `UploadHelperError`, `BackupHelperError`, `NotificationsHelperError`, `FolderMetadataError`, `MountHelperError`, `PublinkHelperError`, `CryptoHelperError`, `AuthHelperError`, `DownloadHelperError`, `CreateFolderHelperError`, `ValueKvError`, `SettingKvError`, `AccountUtilityError` — each wrapped in `SdkError` at `lib.rs:819`. ✓ good structural discipline.

### 8.11 SDK — Examples

`crates/pcloud-sdk/examples/login_and_list.rs` — only example (73 lines). Covers:
- `EmbeddedDaemon::builder(...).build()`.
- `dispatch(Request::Plain { method: GetStatus })`, `GetSyncRoots`.
- Optional `PasswordSubmission` behind `PCLOUD_LIVE=1` gate.

**Missing:** upload example, download example, public-link example, crypto example. M4.

### 8.12 SDK — Tests

`lib.rs:3337-end` — `#[cfg(test)] mod tests`. Tests present (line numbers from grep):
- `embedded_daemon_dispatches_requests` (3369)
- `plugin_registration_is_denied_by_default` (3384)
- `direct_upload_helpers_require_authentication` (3399)
- `upload_data_executes_against_development_transport` (3417)
- `upload_file_reads_local_payload_and_executes_upload` (3440)
- `upload_data_as_resolves_remote_path_before_uploading` (3472)
- `upload_file_as_uses_remote_path_resolution` (3494)
- `upload_data_as_rejects_missing_remote_folder` (3525)
- `download_helpers_resolve_link_and_bytes` (3769)
- `download_helper_funnels_to_api` (4286)
- plus several error/variant tests.

Happy path coverage for `upload_data`, `upload_file`, `upload_data_as`, `upload_file_as`, `download_file` ✓. Missing: test for `create_remote_folder`, `create_remote_folder_by_path`, `check_and_create_folder`, `create_backup`, `send_publink`.

### 8.13 SDK — Feature flags

**H2.** `crates/pcloud-sdk/Cargo.toml` has **no `[features]` section**. Compare with `pcloud-daemon`, `pcloud-observability`, `pcloud-kms`, `pcloud-idp` which all declare `[features] default = ...`. SDK consumers cannot opt into or out of `tls-rustls`/`tls-native`, nor `crypto`, nor `metrics`.

Concrete fix: add `[features] default = ["tls-rustls"]\n tls-rustls = [...]\n tls-native = [...]\n crypto = ["pcloud-crypto"]` with appropriate dep gating. Document the matrix in a `## Feature flags` section in `lib.rs`.

### 8.14 SDK — Missing transfer_api wrappers

`crates/pcloud-proto/src/transfer_api.rs` exposes (pub fns):
- `new`, `apply_api_server_hint`, `get_file_link`, `upload_create`, `upload_delete`, `delete_file`, `rename_file`, `upload_info`, `upload_blockchecksums_begin`, `get_checksum_link`, `encode_upload_write_from_file`, `encode_uploadfile`, `parse_uploadfile_response`.

SDK (`lib.rs`) exposes only high-level wrappers:
- `upload_data`, `upload_file`, `upload_data_as`, `upload_file_as`, `start_upload`, `download_file`, `get_file_link`.

**Gap (H3):** no SDK wrapper for `rename_file`, `delete_file`, `upload_info` (progress probe), `upload_delete` (cancel transient), `upload_blockchecksums_begin` / `get_checksum_link` (dedup flow). An embedded-daemon consumer wanting to rename or delete a remote file has no SDK path and must bypass into `pcloud-proto` directly — breaking encapsulation and making semver brittle.

Concrete fix: add `EmbeddedDaemon::rename_file(file_id, new_name)`, `::delete_file(file_id)`, `::upload_progress(upload_id)`, `::upload_abort(upload_id)`. Lower each into the existing `TransferApi<T>` on the runtime. Add companion error variants under `SdkError`.

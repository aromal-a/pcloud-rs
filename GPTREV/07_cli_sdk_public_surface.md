# GPTREV 07 - CLI And SDK Public Surface Audit

Scope covered `crates/pcloud-cli`, `crates/pcloud-sdk`, SDK examples/tests/benches, relevant IPC/model/proto public types, CLI completion/help/exit/version behavior, feature flags, and CLI coverage against daemon IPC `Request`. No files were modified.

## Findings

### 07-H-01: `pcloudc stat <PATH>` parses the command name as the remote path
- Severity: High
- Evidence: Help advertises `stat <REMOTE-PATH>` at `crates/pcloud-cli/src/app.rs:270`, but the parser reads `args.get(1)` instead of `args.get(2)` at `crates/pcloud-cli/src/app.rs:2445-2458`. The request then sends `inputs.stat_remote_path` as `Request::StatPath` at `crates/pcloud-cli/src/commands.rs:1219-1221`.
- Impact: `pcloudc stat /Documents/report.txt` sends `"stat"` as the remote path and ignores the user-provided path, making the command functionally broken.
- Remediation: Change the parser to read `args.get(2)`, then add regression tests that assert both `stat` and any alias produce `Request::StatPath { path: "/..." }`.

### 07-H-02: Some secret-bearing CLI positionals bypass the explicit argv-risk gate
- Severity: High
- Evidence: Global help warns that argv passwords require `--allow-argv-password` at `crates/pcloud-cli/src/app.rs:163-170`. `change-link-password <ID> [PW]` directly wraps `args.get(3)` into `SecretString` at `crates/pcloud-cli/src/app.rs:1855-1875`. Crypto password rotation directly accepts old/new passphrases from `args.get(2)` and `args.get(3)` at `crates/pcloud-cli/src/app.rs:2612-2645`, while allowed flags omit `--allow-argv-password` at `crates/pcloud-cli/src/app.rs:1076-1080`. Other secret commands correctly enforce the gate at `crates/pcloud-cli/src/app.rs:1645-1712` and `crates/pcloud-cli/src/app.rs:3427-3464`.
- Impact: Public-link passwords and crypto passphrases can leak through shell history and process listings without the explicit acknowledgement required elsewhere.
- Remediation: Remove positional secret support or require `--allow-argv-password` consistently; prefer `--password-stdin` and `--password-env`; add tests that ungated argv secrets fail with usage exit code.

### 07-H-03: SDK semver surface exposes private workspace crate types despite claiming it does not
- Severity: High
- Evidence: The SDK semver docs claim only `upload_session` types and `pcloud_proto::Notification` are exported and that `ConfigProfile`/`Environment` are not re-exported at `crates/pcloud-sdk/src/lib.rs:52-61`. Public signatures expose `Environment` at `crates/pcloud-sdk/src/lib.rs:1245-1258`, `ConfigProfile` at `crates/pcloud-sdk/src/lib.rs:1333-1344`, `Request`/`Response` at `crates/pcloud-sdk/src/lib.rs:1346-1378`, plugin API types at `crates/pcloud-sdk/src/lib.rs:1406-1448`, `pcloud_proto::Notification` at `crates/pcloud-sdk/src/lib.rs:113-117`, and `pcloud_model::public_links::CreatedTreePublicLink` at `crates/pcloud-sdk/src/lib.rs:2481-2486`.
- Impact: Internal config, IPC, plugin, proto, and model crate changes become SDK semver breaks for downstream consumers.
- Remediation: Introduce SDK-owned wrapper/newtype types or explicitly feature-gate/mark raw dispatch and plugin APIs unstable; update the Semver rustdoc to match the actual public contract.

### 07-H-04: SDK `delete_file` and `rename_file` are stale stubs although IPC/daemon support exists
- Severity: High
- Evidence: `delete_file` always returns `"delete_file IPC variant not yet implemented"` at `crates/pcloud-sdk/src/lib.rs:3481-3492`, and `rename_file` always returns `"rename_file IPC variant not yet implemented"` at `crates/pcloud-sdk/src/lib.rs:3506-3516`. IPC already defines `FileDeleteByPath` and `RenamePath` at `crates/pcloud-ipc/src/methods.rs:964-979` and `crates/pcloud-ipc/src/methods.rs:1095-1118`; daemon dispatch handles them at `crates/pcloud-daemon/src/runtime.rs:784-802`.
- Impact: SDK callers get guaranteed failure for advertised file mutation helpers even though the daemon can perform the operations.
- Remediation: Wire the SDK helpers through `Request::FileDeleteByPath` and `Request::RenamePath`, validate absolute paths, and add mocked/runtime tests for success and error mapping.

### 07-M-01: Shell completion clap tree drifts from the real parser
- Severity: Medium
- Evidence: Completion generation is a parallel clap tree, not the runtime parser, by design at `crates/pcloud-cli/src/completion.rs:1-7`. It models `sync change-type` as `<local-path> <two-way|upload-only|download-only>` at `crates/pcloud-cli/src/completion.rs:85-100`, while the parser/help require `<ID> <FLAVOR>` with more aliases at `crates/pcloud-cli/src/app.rs:252-255` and `crates/pcloud-cli/src/app.rs:1753-1770`. It exposes `sync suggest --limit` at `crates/pcloud-cli/src/completion.rs:109-115`, but the parser accepts `--max` at `crates/pcloud-cli/src/app.rs:1061-1063` and `crates/pcloud-cli/src/app.rs:2648-2658`. It omits `upload write-from-file` even though the parser and command mapping support it at `crates/pcloud-cli/src/app.rs:865-872` and `crates/pcloud-cli/src/commands.rs:1317-1323`. It omits `verify --yes`, accepted at `crates/pcloud-cli/src/app.rs:1034-1036`.
- Impact: Generated completions mislead operators and hide supported commands/flags.
- Remediation: Generate completions from a single command metadata source, or add tests that compare parser-recognized commands/flags against `completion::build_cli`.

### 07-M-02: `--flag=value` is accepted by validation but ignored by most value parsers
- Severity: Medium
- Evidence: Unknown-flag validation explicitly accepts `--flag=value` by splitting on `=` at `crates/pcloud-cli/src/app.rs:1208-1217`. The common extractor `parse_flag_string` only handles `--flag value` and ignores inline values at `crates/pcloud-cli/src/app.rs:3158-3168`. Affected call sites include `sync add --type` at `crates/pcloud-cli/src/app.rs:1743-1746`, `sync suggest --max` at `crates/pcloud-cli/src/app.rs:2651-2655`, and crypto setup `--backend`/`--hint` at `crates/pcloud-cli/src/app.rs:2991-3028`.
- Impact: Commands like `sync add /a /b --type=backup` pass flag validation but silently use defaults or prompt unexpectedly.
- Remediation: Teach `parse_flag_string` and numeric helpers to handle `strip_prefix("--flag=")`, then add paired tests for spaced and inline forms.

### 07-M-03: Path-based tree public link API advertises mixed paths but treats every path as a folder
- Severity: Medium
- Evidence: CLI help describes `create-tree-link <PATHS...>` as "mixed files" at `crates/pcloud-cli/src/app.rs:311-316`. SDK docs/examples show file and folder paths together at `crates/pcloud-sdk/src/lib.rs:2444-2480` and `crates/pcloud-sdk/examples/create_tree_public_link_from_paths.rs:2-8`. Implementation sets `folders: paths` and `files: vec![]` at `crates/pcloud-sdk/src/lib.rs:2502-2509`, while lower-level IPC supports both `folder_ids_csv` and `file_ids_csv` at `crates/pcloud-ipc/src/methods.rs:500-510`.
- Impact: File paths in the advertised path API cannot be included correctly, so mixed tree links fail or silently exclude file semantics.
- Remediation: Resolve each path to file vs folder before building `TreePublicLinkPaths`, or rename/document the API as folder-only until file resolution exists.

### 07-M-04: SDK upload conflict mode is public but ignored in the legacy upload path
- Severity: Medium
- Evidence: `ConflictMode` is a public enum documented as mapping to the C `ifhash` family at `crates/pcloud-sdk/src/upload_session.rs:84-93`, and `UploadRequest` exposes `conflict_mode` at `crates/pcloud-sdk/src/upload_session.rs:173-200`. `run_upload` discards it with `let _ = &request.conflict_mode` at `crates/pcloud-sdk/src/upload_session.rs:708`. Tests only assert the builder accepts `IfHashNumeric`, not that it affects wire behavior, at `crates/pcloud-sdk/src/upload_session.rs:831-840`.
- Impact: SDK consumers may believe create-if-absent or conditional-overwrite semantics are enforced when they are not.
- Remediation: Thread conflict mode into the upload wire/API layer or remove/mark it unavailable until supported; add a transport-level test that verifies the emitted request includes the intended conflict policy.

### 07-M-05: CLI/SDK READMEs are stale relative to the actual public surface
- Severity: Medium
- Evidence: CLI README says commands are parsed with clap and shows `pcloudc login --email` and `sync add --local --remote` at `crates/pcloud-cli/README.md:8` and `crates/pcloud-cli/README.md:20-23`; actual help uses `-u/--user/--username` and positional `sync add <LOCAL> <REMOTE>` at `crates/pcloud-cli/src/app.rs:115-119` and `crates/pcloud-cli/src/app.rs:235-240`. SDK README advertises `Sdk::new`, `Sdk::login`, `Sdk::upload_file`, `Sdk::upload_data`, and `pcloud_sdk::Error` at `crates/pcloud-sdk/README.md:13-26`; actual exported entry point is `EmbeddedDaemon`/`EmbeddedDaemonBuilder` at `crates/pcloud-sdk/src/lib.rs:119-135` and `crates/pcloud-sdk/src/lib.rs:1307-1318`.
- Impact: First-run instructions and code snippets are not compile/run accurate, which is a public-surface readiness defect.
- Remediation: Rewrite README examples against current CLI flags and SDK `EmbeddedDaemon` APIs; add README snippet tests where feasible.

### 07-M-06: SDK feature flags do not expose enterprise TLS backend choice
- Severity: Medium
- Evidence: `crates/pcloud-sdk/Cargo.toml:9-20` documents that `tls-native` is not implemented and that `pcloud-proto` hard-pins rustls plus webpki roots. The SDK feature table has only `default = []` at `crates/pcloud-sdk/Cargo.toml:22-27`.
- Impact: Enterprise embedders cannot select platform-native trust stores or alternative TLS policy through SDK features.
- Remediation: Add explicit `tls-rustls` and `tls-native` feature plumbing through `pcloud-proto`, document the current rustls-only limitation, and add CI for both configurations once implemented.

### 07-M-07: CLI command coverage does not classify reachable daemon IPC variants
- Severity: Medium
- Evidence: IPC/daemon expose per-root pause/resume at `crates/pcloud-ipc/src/methods.rs:394-404` and `crates/pcloud-daemon/src/runtime.rs:637-639`, typed key/value operations at `crates/pcloud-ipc/src/methods.rs:632-659` and `crates/pcloud-daemon/src/runtime.rs:751-753`, and path VFS operations at `crates/pcloud-ipc/src/methods.rs:960-1118` with daemon handling at `crates/pcloud-daemon/src/runtime.rs:784-802`. The CLI has broad sync/global pause commands but no clear CLI route for these specific IPC surfaces.
- Impact: Operator-scriptable coverage is uneven and undocumented; some daemon functionality is only reachable through raw SDK/IPC paths.
- Remediation: Maintain a generated coverage matrix classifying each `Request` as CLI, SDK-only, plugin-only, or internal; add CLI commands for operator-relevant gaps or document why each is intentionally excluded.

## Verified Areas

- Rustdoc coverage: `pcloud-sdk` has `#![deny(missing_docs)]` at `crates/pcloud-sdk/src/lib.rs:78`; CLI main also denies missing docs at `crates/pcloud-cli/src/main.rs:15`. `cargo doc -p pcloud-sdk -p pcloud-cli --no-deps` passed.
- Exit codes: Stable exit-code mapping is documented and tested at `crates/pcloud-cli/src/exit_code.rs:24-35` and `crates/pcloud-cli/src/exit_code.rs:140-212`.
- Version reporting: `--version` includes package version, git hash, and build profile via `crates/pcloud-cli/src/main.rs:77-90`; build metadata injection is in `crates/pcloud-cli/build.rs:20-53`.

## Commands Run

- `sed -n '1,240p' pcloud_rev.md`
- `rg --files crates/pcloud-cli crates/pcloud-sdk crates/pcloud-model`
- `rg -n "Request::|Method::" crates/pcloud-daemon/src/runtime.rs`
- `rg -n "TODO|FIXME|STUB|stub|unimplemented|panic!|not yet implemented|Unavailable|not_configured" crates/pcloud-cli crates/pcloud-sdk --glob '!target/**'`
- `rg -n "pub (use|type|struct|enum|fn)|pub fn|pub const|pub mod" crates/pcloud-sdk/src/lib.rs crates/pcloud-sdk/src/upload_session.rs crates/pcloud-model/src/*.rs`
- `cargo test -p pcloud-cli -p pcloud-sdk --all-targets --no-run` passed
- `cargo test -p pcloud-cli -p pcloud-sdk --all-targets` passed
- `cargo test -p pcloud-sdk --all-targets --no-default-features` passed
- `cargo test -p pcloud-sdk --all-targets --all-features` passed
- `cargo test -p pcloud-cli --all-targets --no-default-features` passed
- `cargo doc -p pcloud-sdk -p pcloud-cli --no-deps` passed

## Limitations

No live pCloud account or network E2E was used; SDK live examples guarded by `PCLOUD_LIVE=1` were compiled/tested but not executed against the service. I did not audit daemon/proto internals beyond the IPC/model/proto types needed for CLI/SDK public-surface findings. Generated directories, `target/`, `vendor/`, `.beads/`, and tracker output were excluded.

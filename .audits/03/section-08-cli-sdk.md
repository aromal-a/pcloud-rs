# Section 8: CLI & SDK Surface
## Date: 2026-04-17
## Auditor: Claude Opus (Agent 8)

Scope reviewed:
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/` (app.rs 4702 LOC, main.rs 2299 LOC, commands.rs 1448 LOC, completion.rs 285 LOC, exit_code.rs 212 LOC, prompt.rs 251 LOC, globals.rs 807 LOC, build.rs, config.rs, doctor.rs, field_selector.rs, json_output.rs, migrate.rs, output.rs, progress.rs, verify.rs)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/` (lib.rs 4437 LOC, upload_session.rs 847 LOC, four examples, one tests/upload_session_chunked.rs)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs` (Request enum cross-reference)
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv` rows 172-187 (CLI/SDK coverage)
- Workspace `Cargo.toml`, per-crate `Cargo.toml` for feature flags

## Findings

### CRITICAL [0]

No CRITICAL findings. All identified issues are HIGH or lower.

### HIGH [3]

#### H-1. Argv password accepted without `--allow-argv-password` on `submit-auth`, `crypto start`/`unlock-crypto`, `submit-tfa`, `submit-recovery`
Severity: HIGH
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs:1526-1552`

Only `Command::SubmitPassword` gates argv-supplied credentials behind the `--allow-argv-password` opt-in (see `read_password_securely` at `app.rs:2997-3013`). The parallel paths that accept secrets positionally do NOT:
- `SubmitAuthToken` (`submit-auth TOKEN`): line 1526, takes `args.get(2)` directly into `SecretString` with no warning, no gate.
- `SubmitCryptoPassword` (`crypto start PW` / `unlock-crypto PW`): line 1544, same pattern.
- `SubmitTwoFactorCode` / `SubmitRecoveryCode` (`tfa CODE` / `submit-tfa CODE` / `submit-recovery CODE`): line 1535, same pattern. TFA codes are short-lived but still leak via `/proc/<pid>/cmdline` until process exit and survive in shell history.

Security rules in `CLAUDE.md` require: "keep secret-bearing CLI input off stdout/history where possible". The gated path on `submit-password` already sets the correct pattern; sibling paths diverge.

Remediation: extend `read_password_securely` (or a parallel helper) to enforce `--allow-argv-password` and the accompanying stderr warning for `submit-auth`, `unlock-crypto`/`crypto start`, `submit-tfa`, `tfa`, and `submit-recovery`. Add symmetric tests under `app.rs` tests mod mirroring the `hunter2` test at line 4201.

#### H-2. SDK public surface re-exports `pcloud_proto::Notification` — couples SDK semver to protocol crate
Severity: HIGH
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:105`

`pub use pcloud_proto::Notification;` — `pcloud-proto` is an internal protocol crate (not `publish.workspace = true` + intended as a stable SDK layer). This binds every `pcloud-sdk` consumer to the upstream protocol type; any change in `pcloud_proto::Notification` is an automatic SDK breaking change.

The audit brief explicitly calls out: "Public API surface is semver-disciplined (no `pub use` of internal types that would bind the caller to private crates)". All 15 other `pub struct` / `pub enum` types in `lib.rs` (`UploadResult`, `PromoResult`, `ApiServerResult`, `FilesystemPathStatus`, `StatResult`, `FolderEntry`, etc.) are defined locally in the SDK — only `Notification` is re-exported.

Remediation: define a `pub struct Notification { ... }` inside `pcloud-sdk` with the stable field shape the SDK wants to expose and convert from `pcloud_proto::Notification` at the helper call site (`list_notifications` at line 2218). This decouples SDK semver from protocol-crate churn.

#### H-3. `UploadHelperError::ReadLocalFile` shadows the `SdkError::Io` variant via duplicate `#[from] std::io::Error`
Severity: HIGH
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:277-290` and `916-918`

Both `UploadHelperError` (variant `ReadLocalFile(#[from] std::io::Error)`) and `SdkError` (variant `Io(#[from] std::io::Error)`) accept `std::io::Error` via `From`. Because `UploadHelperError: From<io::Error>` and `SdkError: From<UploadHelperError>` and `SdkError: From<io::Error>`, the `?`-chain in helpers that call `std::fs::read(local_path)?` (e.g. `upload_file` at lib.rs:1460) routes `io::Error` through `SdkError::Io` directly instead of the semantically correct `SdkError::Upload(UploadHelperError::ReadLocalFile(...))`. Errors lose their "this is an upload local-read failure" classification and the unified-error category maps to `Category::LocalIo` with no upload context.

Observed code at line 1460: `let bytes = std::fs::read(local_path)?;` — this `?` uses `From<io::Error> for SdkError` (direct path), bypassing the `ReadLocalFile` wrapping the docs promise.

Remediation: either remove `#[from] std::io::Error` from `SdkError::Io` (keep the variant, make it explicit) or funnel local-file reads inside the upload helpers through an explicit `.map_err(UploadHelperError::ReadLocalFile)?`. Add a unit test that asserts `upload_file(…, nonexistent_path)` returns `SdkError::Upload(UploadHelperError::ReadLocalFile(_))` — today it would fail.

### MEDIUM [6]

#### M-1. `FileMutationHelperError`, `delete_file`, `rename_file`, `get_file_info` — explicitly listed in audit brief but absent from SDK
Severity: MEDIUM
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs` (absence)

Grep confirms: no `FileMutationHelperError` symbol, no `delete_file` / `rename_file` / `get_file_info` functions. The only `delete_*` methods are `delete_backup`, `delete_backup_device`, `delete_public_link` (via IPC). No file-level CRUD helper exists.

This is a gap against the expected SDK surface. C parity has `psync_delete` / `psync_rename` (file API family) that the matrix does not currently surface.

Remediation: add `delete_file(file_id)`, `rename_file(file_id, new_name)`, `get_file_info(file_id)` helpers plus a `FileMutationHelperError` enum. If intentionally deferred, mark the rows in `C_FEATURE_PARITY_MATRIX.csv` as `Missing` with a bead reference (likely `bd-1du.10`).

#### M-2. Shell-completion generator omits most subcommands and all flags
Severity: MEDIUM
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/completion.rs:72-192`

`build_cli()` lists ~60 subcommands but the legacy parser accepts far more (per `parse_single_token` at `app.rs:1302-1474` and `normalize_args` at `app.rs:532-900`). Subcommands such as `integrity status`, `integrity run-once`, `integrity skip`, `upload create`, `upload pause`, `conflict list`, `conflict resolve`, `audit-verifier`, `ha`, `slo`, `stat`, `reload`, `drain`, `snapshot create/restore/verify/prune`, `log` (file history), `diff`, `restore`, `verify`, `migrate-from-c`, `backup delete`, `backup snapshot-*` are **not** represented in the completion tree. Also: no flag completions (`--json`, `--output`, `--field`, `--password-stdin`, etc.) and no positional-value hints.

The tests at `completion.rs:226-238` only assert the tree has `status|health|sync|crypto|completion|finalize`, which is why the gap has not been caught.

Remediation: treat `build_cli()` as the single source of truth; add every subcommand currently accepted by `parse_single_token` plus global/per-command flags from `globals::known_flag_names()` and `allowed_flags_for`. Add a golden-file test that round-trips each `Command` variant to guarantee the completion tree stays in sync with new subcommands.

#### M-3. No `tls-rustls` / `tls-native` feature-flag choice; TLS backend hard-pinned to rustls+ring
Severity: MEDIUM
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/Cargo.toml:24`

Audit brief: "Feature flags (`default-features`, `tls-rustls` vs `tls-native`, etc.) — combinations all compile?" The workspace has exactly one TLS choice: `rustls = { version = "0.23", default-features = false, features = ["std", "ring"] }` hard-coded in `pcloud-proto`. No `[features]` section offers `tls-rustls` vs `tls-native-tls` vs `aws-lc-rs`. Enterprise consumers who need FIPS-validated native TLS or to bypass ring (licensing/export reasons) have no way to opt in.

Remediation: add `[features] tls-rustls-ring = [...]` (default), `tls-rustls-aws-lc = [...]` (FIPS path), `tls-native = [...]` (system), gate the `rustls` / `webpki-roots` deps with `optional = true`, and exercise each combo in CI via `cargo check --no-default-features --features tls-*`.

#### M-4. CLI unit test coverage is strong but `main.rs::run` integration-level behaviour (dispatch, JSON envelope, error classification paths) is not tested
Severity: MEDIUM
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/main.rs:104-506`

`exit_code.rs` unit-tests are thorough (tests module lines 140-212). `commands.rs` tests the `into_request` dispatch (lines 1273-1448). Parsing tests are abundant in `app.rs`. But `main::run()` — the 400-line dispatcher that branches on command, drives the IpcClient, renders JSON envelopes, handles `--field` projection, and classifies transport errors via `ExitCode::classify_transport_error` — has no test module. The `tests/` directory has `field_selector_cli.rs` (field projection only), `migrate_fixture.rs`, `upload_session.rs` — nothing covering the exit-code path from IPC error → rendered output → returned `ExitCode`.

Remediation: add an integration test under `crates/pcloud-cli/tests/run_dispatcher.rs` that stubs the IpcClient (already possible — `send` takes `&Path`) or spawns a scratch IPC socket to assert end-to-end behaviour for at least: success path, `Unauthorized` → exit 3, transport fail → exit 4, `PolicyViolation` → exit 7, JSON envelope shape.

#### M-5. SDK helper docs claim "secret-bearing CLI input" hygiene but several public helpers take `&str`/`String` passwords rather than `SecretString`
Severity: MEDIUM
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:1764-1842`

`change_password` (line 1764), `register` (line 1809) take their password parameters as plain types (`impl Into<String>` / `&str`) — examined via the signatures, they ultimately wrap into `SecretString` inside the body, but the public API lets callers hold the plain `String` on the stack before the SDK call. `crypto_change_password` at line 1940 and `crypto_change_password_unlocked` at 1992 are similar.

In contrast, `Command::SubmitPassword` dispatch at `commands.rs:907-913` uses `SecretString` on the CLI side. The SDK surface should not regress.

Remediation: change the public signatures to `password: SecretString` / `old: SecretString, new: SecretString`. This forces callers to zeroize; the crate already re-exports `pcloud_secret::secret_string::SecretString` transitively.

#### M-6. SDK public function doc-comment discipline: most have examples, but ~15 helpers lack them
Severity: MEDIUM
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs` (various)

Helpers without runnable `///` examples include (grep-verified): `start_upload` (1377), `crypto_priv_key_flags` (1860), `crypto_send_change_user_private` (1875), `set_backup_device_folder_id` (2197), `run_localscan` (2275), `backup_device_folder_id` (2531), several `get_*_setting` / `set_*_setting` pairs. Audit brief: "Every public fn has a doc comment with an example (if not trivial)".

Remediation: add `/// ```no_run` blocks for each listed helper. For getters that return `Option<_>` / primitives, a single-line `let x = d.method();` suffices and satisfies the clippy `missing_doc_code_examples` lint (not enabled but targeted by the audit rule).

### LOW [5]

#### L-1. Help text contains `TODO(bd-xplat)` markers — Linux-only idioms leaked into user-visible help
Severity: LOW
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs:23`, `:160`

`help_text()` (concat! block) embeds `// TODO(bd-xplat)` comments between literal strings describing Unix socket / `/proc/<pid>/environ` behaviour. The comments compile out but mark known gaps that are surfaced to the user as confident claims in the help text. Remediation: either add explicit `(Linux)` qualifiers in the user-visible lines, or gate the affected paragraphs with `#[cfg(target_os = "linux")]` once `app` is moved off a single `const` help string.

#### L-2. `env_force_umount_enabled` is an environment-variable override that the `--help` text never documents
Severity: LOW
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/commands.rs:1266-1271`

`PCLOUD_FORCE_UMOUNT=1 pcloudc unmount` promotes a plain unmount to a `MountForceUnmount` IPC. Not mentioned in `help_text()` or the "FILESYSTEM MOUNT" section (app.rs:207-222). Undocumented env toggles are an enterprise-ops anti-pattern. Remediation: document in the help text or remove in favour of an explicit `--force` flag.

#### L-3. `version_banner` reports `pcloudc <version> (<git-hash>, <profile>)` but the profile string is lifted from `$PROFILE` which cargo sets to `debug`/`release` only
Severity: LOW
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/main.rs:56-66` and `build.rs:22`

When users build with `--profile release-dist` or `--profile release-repro`, `$PROFILE` is still `"release"` (the compile-time env var cargo exposes). `--version` therefore cannot distinguish a `release-dist` from `release-repro` binary, which matters for reproducibility verification. Remediation: also read `$CARGO_CFG_TARGET_ENV` or capture the profile via `env::var("CARGO_PROFILE")` in a forward-compat way, or inject it from CI via `cargo:rustc-env=BUILD_PROFILE=release-dist`.

#### L-4. `SdkError` is `#[non_exhaustive]` but inner helper enums are also `#[non_exhaustive]` — double annotation is redundant and slightly cumbersome for consumers
Severity: LOW
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:258-405` (all helper enums) + 817

Not a bug, but double non-exhaustive forces downstream code to write `_ => …` arms at two levels. The outer `SdkError` is the surface contract; most inner enums are effectively detail. Remediation: keep `#[non_exhaustive]` on `SdkError` only and let helper enums declare their full variant set — unless a helper is known to grow, the inner `#[non_exhaustive]` adds friction without value.

#### L-5. `SdkError::Io` variant allows callers to receive `io::Error` for local-file reads without the `UploadHelperError::ReadLocalFile` wrapper — inconsistent with docs
Severity: LOW
File: `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:909-918` (tie-in with H-3)

Already flagged as HIGH for the concrete upload path. The LOW angle: the `Io` variant docs say "Local I/O failure surfaced directly (e.g. reading an upload payload)" — that claim contradicts the `UploadHelperError::ReadLocalFile` variant which is documented as the channel for identical failures. Remediation covered by H-3.

## Positive findings (audit-required confirmations)

- **Exit-code discipline**: `ExitCode` enum in `exit_code.rs:58-87` documents 9 stable numeric codes, carries a `# Stable-ABI guarantee` section, is printed via `EXIT_CODE_HELP`, and `tests/` verify the `ResponseStatus → ExitCode` mapping is intact (`exit_code.rs:140-212`). Mapping covers `Auth` (3) vs `Network` (4) vs `Conflict` (7) vs `Internal` (8) vs `Unavailable` (6) — matches audit brief expectations.
- **Argv password gate on `submit-password`**: `app.rs:2997-3013` hard-fails with a message pointing at `--password-stdin` / `--password-env` / `--allow-argv-password`, exits 2. Matches enterprise expectation.
- **Interactive prompt hygiene**: `prompt.rs:146-246` masked-TTY prompt uses a `Restore` RAII guard to re-apply termios on every exit path including panic. Falls back to `rpassword` on non-TTY stdin. Zeroization deferred to `SecretString` at the callsite (`commands.rs:568`).
- **`pcloudc --version`**: `main.rs:56-66` emits `pcloudc <pkg-version> (<git-hash>, <profile>)`. `build.rs:20-54` soft-fails gracefully when git is absent. Audit brief item satisfied.
- **Shell-completion backends**: bash, zsh, fish, elvish, powershell all supported via `clap_complete` (`completion.rs:196-213`) and each has a `_non_empty` unit test.
- **Every CLI `Command` variant has an `into_request` arm**: `commands.rs:752-1253` is total — pattern coverage verified; no panic fallback. CLI-side-only variants (`Start`, `Drain`, `Reload`, `Doctor`, `MigrateFromC`, `FileDiff`, `FileRestore`) route to harmless `GetHealth`/`DrainStatus` probes as documented defensive fallbacks.
- **CLI parity matrix rows 172-186** (14 rows) all read `Implemented` — legacy `sync add/remove/pause/resume`, `crypto start/stop`, `tfa`, `auth`, `authsave`, `finalize`, `quit`, `help`, `status`, `pending` — all resolve through `parse_command` → `Command::into_request`.
- **SDK examples**: all four (`login_and_list`, `upload_and_download`, `public_link`, `crypto_lifecycle`) are gated on `PCLOUD_LIVE=1` (with `PCLOUD_USERNAME/PCLOUD_PASSWORD/PCLOUD_EMAIL/PCLOUD_CRYPTO_PASS` as required) and print a dry-run hint otherwise — safe in CI. Each compiles through `[[example]]` declarations in `pcloud-sdk/Cargo.toml:29-43`.
- **SDK `SecretInputs` hygiene**: `commands.rs:565-748` holds every secret-bearing field as `SecretString`, with an explicit note that `Clone`/`PartialEq` are NOT derived (audit M3 hardening) — forces callers through `clone_secret()` at audit-visible sites.
- **`SdkError` is the consolidated error type**: single `pub enum` at line 817 wraps all 14 helper errors with `#[non_exhaustive]` — callers see `SdkError` only. Matches semver-disciplined error-surface expectations.

## Summary

3 HIGH findings (argv-secret gate gap, proto re-export, duplicate `#[from] io::Error`), 6 MEDIUM (missing FS-mutation helpers, completion tree gaps, TLS feature flexibility, integration-test coverage, public-API secret types, doc examples), 5 LOW (mostly cosmetic / docs). The CLI core (parsing, exit codes, argv → IPC dispatch) is solid and well-tested. The SDK surface is substantially complete with documented error taxonomy, but benefits from three tightening changes (H-2 proto decoupling, H-3 error-path correctness, M-1 file-mutation helpers) before it can claim stable-public-API readiness.

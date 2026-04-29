# Audit 08 — CLI & SDK Surface

Scope: `crates/pcloud-cli/` (binary `pcloudc`) and `crates/pcloud-sdk/`.
Read-only audit. Cross-referenced against `pcloud_rev.md` §8 and CLAUDE.md security rules.

## Summary

Both crates are in good shape relative to the master prompt. The CLI has a
hand-written help text that is exhaustive and accurate, exit codes are
formally enumerated with a stable-ABI guarantee and unit-tested round-trip
mapping, the prompt/termios path is RAII-correct, and `--version` produces
`pcloudc <pkg-version> (<git-hash>, <profile>)` with build provenance
warnings in release builds when `GIT_HASH` is missing. The SDK enforces
`#![deny(missing_docs)]` and `#![forbid(unsafe_code)]`, has 46 unit tests in
`lib.rs` covering the helper happy paths, all 5 examples build clean, and
`cargo doc -p pcloud-sdk --no-deps` is warning-free at the public surface.

The defects found are mostly polish items (broken intra-doc links inside
private items, missing feature flags advertised in the master prompt, a
hidden-stub UX wart on `diff`/`restore`), plus one structural risk: the
hand-rolled `clap::Command` tree in `completion.rs` is a parallel
description of the legacy parser, so it can drift silently from the actual
parser in `app.rs`. The CLI binary does **not** route argv through clap at
runtime — clap is used only to generate completions, so any divergence
ships to users without a compile error.

Severity counts: CRITICAL 0 · HIGH 1 · MEDIUM 5 · LOW 4.

---

## Findings

### HIGH-08-1 — Completion CLI tree is a parallel description, not the source of truth

- **Severity:** HIGH (drift risk; semver and UX correctness)
- **File:** `crates/pcloud-cli/src/completion.rs:35-652`
- **Evidence:** `build_cli()` constructs a `clap::Command` tree by hand
  to feed `clap_complete::generate`. The runtime parser is a hand-rolled
  legacy tokeniser in `crates/pcloud-cli/src/app.rs` (`normalize_args`,
  `parse_command`, ~5400 lines of manual matching). The module docstring
  (line 4-6) explicitly notes "This does NOT replace the legacy token
  parser — we build a parallel, descriptive `clap::Command` tree". Spot
  check: `app.rs:483-515` lists 18 value-taking flags (`--to`,
  `--message`, `--from`, `--user`, `--username`, `--tfa-channel`,
  `--channel`, `--password-env`, `-m`, `--mountpoint`, `--fuse-opts`,
  `--log-path`, `--fs-event-log`, `--log-level`, `--cache-size`,
  `--config`, `--limit`, `--gpg-recipient`, `--retention-days`,
  `--zstd-level`, `--type`, `--max`, `--backend`, `--hint`); only a
  fraction surface in the completion tree (`completion.rs:42-71` for
  globals, plus per-subcommand args). `pcloudc help` itself is also a
  hand-rolled `concat!()` block in `app.rs:16-466`. Three independent
  surfaces (legacy parser, help text, completion tree) describe the
  same CLI and can drift.
- **Risk:** A new flag added to `app.rs` is invisible to tab-completion
  unless the developer remembers to update `completion.rs`; conversely,
  `completion.rs` advertises subcommands users can tab-complete that the
  parser may reject. There is no compile-time link between the three
  descriptions. `pcloud_rev.md` §8 requires "every subcommand's help
  matches the actual behavior" — there is no test that enforces this.
- **Remediation:** Add a regression test that builds the completion-tree
  subcommand list, builds the help-text subcommand list (regex-extract
  from `app::help_text()`), and asserts both against a canonical
  `Command` enum dump (e.g. iterate `Command::variants()` or hand-curate
  a frozen list and break it on diff). Long-term, plan migration of the
  runtime parser to clap derive so all three descriptions are one tree.
  Track under a new bead under `bd-1du`.

---

### MEDIUM-08-2 — Master prompt requires `tls-rustls` vs `tls-native` SDK feature flags; only rustls is wired

- **Severity:** MEDIUM
- **File:** `crates/pcloud-sdk/Cargo.toml:9-27`, `crates/pcloud-proto/Cargo.toml:9-26`
- **Evidence:** Master prompt §8:221 calls out "Feature flags
  (`default-features`, `tls-rustls` vs `tls-native`, etc.) — combinations
  all compile". `pcloud-sdk/Cargo.toml:9-20` contains a TODO(bd-1du)
  acknowledging that "tls-native feature flag not implemented;
  pcloud-proto hard-pins rustls + ring with no way for SDK consumers to
  opt into native-tls". `pcloud-proto/Cargo.toml:26` has
  `rustls = { version = "0.23", default-features = false, features = ["std", "ring"] }`
  as an unconditional dependency — there is no `tls-rustls` feature gate,
  and no `tls-native` alternative. `pcloud-sdk/Cargo.toml:23-27` declares
  `default = []` with no TLS-related flags. `cargo build -p pcloud-sdk
  --no-default-features` succeeds (verified during audit) because there
  is nothing to disable.
- **Risk:** SDK callers cannot opt into `native-tls` for environments
  that require system-store certificate validation (Windows enterprise
  CAs, macOS Keychain trust roots, custom corporate CAs not in
  webpki-roots). The master prompt's "all combinations compile" check
  cannot be satisfied because the combinations don't exist.
- **Remediation:** Either (a) implement the gating per the TODO comment
  in `pcloud-sdk/Cargo.toml:11-19` and add a CI matrix step that builds
  `--features tls-rustls` and `--features tls-native --no-default-features`,
  or (b) close the gap honestly by updating the master prompt and `STATUS.md`
  to declare rustls-only as the supported posture. Option (a) is preferred
  given the documented enterprise requirement.

---

### MEDIUM-08-3 — SDK rustdoc emits two unresolved intra-doc-link warnings

- **Severity:** MEDIUM
- **File:** `crates/pcloud-sdk/src/upload_session.rs:15`,
  `crates/pcloud-sdk/src/upload_session.rs:42`
- **Evidence:** Running `RUSTDOCFLAGS="-D missing-docs" cargo doc -p
  pcloud-sdk --no-deps --document-private-items 2>&1` yields:
  ```
  warning: unresolved link to `pcloud_backends::upload_journal::UploadJournal`
  warning: unresolved link to `DaemonSessionDriver`
  warning: `pcloud-sdk` (lib doc) generated 2 warnings
  ```
  `pcloud_backends` is a dev-dependency in `pcloud-sdk/Cargo.toml:47`,
  not a runtime dependency — so a public-facing intra-doc link to it
  cannot resolve in the published doc. `DaemonSessionDriver` does not
  exist in this fork (likely a pre-rename reference).
- **Risk:** Broken docs ship to docs.rs / cargo doc consumers. Master
  prompt §8:218-219 requires `cargo doc --workspace --no-deps` to be
  warning-free at the SDK layer.
- **Remediation:** Replace the `[`pcloud_backends::upload_journal::UploadJournal`]`
  link with plain text or `pcloud-backends::upload_journal::UploadJournal`
  (no link). Update the `[`DaemonSessionDriver`]` link to the actual
  current type name (`UploadSessionDriver`?) or remove it.

---

### MEDIUM-08-4 — `pcloud-sdk` re-exports `pcloud_proto::Notification` as a transparent type alias, coupling SDK semver to a private crate

- **Severity:** MEDIUM
- **File:** `crates/pcloud-sdk/src/lib.rs:113-117`
- **Evidence:**
  ```rust
  /// Typed notification record mirroring the C `psync_notification_t`. Re-exported
  /// from `pcloud-proto` so SDK consumers do not need a direct dependency on the
  /// protocol crate.
  // NOTE: aliases pcloud_proto::Notification; if that type changes, this is a semver break
  pub type Notification = pcloud_proto::Notification;
  ```
  The crate's own §Semver section in `lib.rs:52-61` correctly states the
  rule: "Any future public re-export of a private-crate type must be
  wrapped in an SDK-owned newtype or alias, and documented here". The
  `pub type` itself violates that rule — a type alias is not a newtype,
  so any structural change in `pcloud_proto::Notification` (added field,
  changed visibility, added derive bound) directly breaks SDK consumer
  semver. Master prompt §8:217 explicitly forbids this: "no `pub use`
  of internal types that would bind the caller to private crates".
  `pub type` has the same binding effect as `pub use` for this purpose.
- **Risk:** A breaking change in `pcloud-proto` cascades into a
  `pcloud-sdk` major-version bump silently. The doc comment marks this
  as a known risk but the code does not enforce containment.
- **Remediation:** Wrap `pcloud_proto::Notification` in an SDK-owned
  `pub struct Notification { /* fields mirror pcloud_proto::Notification */ }`
  with a `From<pcloud_proto::Notification>` impl, or — if
  `pcloud_proto::Notification` is genuinely a stable shared type —
  promote it to `pcloud-model` (which is already a public stable crate)
  and re-export from there.

---

### MEDIUM-08-5 — Hidden `diff` / `restore` subcommands degrade UX trust

- **Severity:** MEDIUM
- **File:** `crates/pcloud-cli/src/completion.rs:599-636`,
  `crates/pcloud-cli/src/app.rs` (`Command::FileDiff`, `Command::FileRestore`)
- **Evidence:** Two subcommands are advertised in `commands.rs` as
  `Command::FileDiff` / `Command::FileRestore` but `completion.rs:610`
  and `:624` apply `.hide(true)` so they don't tab-complete. The
  `about` strings literally end in "(stub — Unavailable)". They always
  exit `Unavailable`. The comment block at `completion.rs:599-609`
  explains the rationale (pCloud public API doesn't expose revision
  diff/restore) and references follow-up `pcloud-rs-07o`.
- **Risk:** Users who learn the CLI from `app::help_text()` or scripts
  that hard-code these tokens will hit `ExitCode::Unavailable`. CLAUDE.md
  Final Rule says "claims must match reality" — shipping a command
  whose name is a lie (it does not diff and does not restore) is a
  parity-truth violation, even if the docs say "stub".
- **Remediation:** Either (a) remove the subcommands entirely (the
  daemon-side stub and the CLI-side handler) and document the gap in
  `STATUS.md`, or (b) keep them only behind an explicit
  `--unstable-stubs` global flag so they're not in the default surface.
  Track follow-up `pcloud-rs-07o`.

---

### MEDIUM-08-6 — `--allow-argv-password` exists only behind a stderr warning, no audit-log signal

- **Severity:** MEDIUM
- **File:** `crates/pcloud-cli/src/app.rs:1631-1711, 3431-3447`,
  `crates/pcloud-cli/src/completion.rs:240-244`
- **Evidence:** When the user passes `--allow-argv-password`, the CLI
  emits a one-shot stderr warning ("WARNING: password supplied on
  argv... `/proc/<pid>/cmdline`... acknowledged.") and then proceeds.
  The daemon receives only the password via the IPC `PasswordSubmission`
  request — there is no auditable "this was an argv-leaked password
  submission" flag in the request envelope and no entry in the audit
  chain that records the operator chose the unsafe path. CLAUDE.md
  Security Rules §"Auth token persistence" require persistence/audit
  failures to be surfaced rather than swallowed; the same discipline
  should apply to a deliberately weakened auth ingress.
- **Risk:** A compromised host can extract the argv password and the
  audit log shows nothing distinguishable from a TTY-prompt login. Post-
  incident forensics cannot prove which login surface was used.
- **Remediation:** Plumb a `secret_provenance: ArgvAcknowledged | Stdin
  | Env | Tty` field into the IPC `PasswordSubmission` (and analogous
  variants for crypto/auth-token) and record it in the audit chain.

---

### MEDIUM-08-7 — `--password-env` env-var scrub uses `unsafe std::env::remove_var` with single-thread invariant unenforced

- **Severity:** MEDIUM
- **File:** `crates/pcloud-cli/src/main.rs:2254-2291`,
  `crates/pcloud-cli/src/app.rs:3415-3423`
- **Evidence:** The env-var scrub path holds a 16-line SAFETY comment
  asserting "This code path executes before the Tokio runtime is
  started — no async task pool has been created" and "No rayon or
  `std::thread::spawn` threads have been spawned by the CLI before
  this point". The invariant is documented but not enforced. There
  is no `assert!(thread::current_thread_count() == 1)` guard, and any
  refactor that moves auth to after a transport handshake (which today
  is an IPC `client.send` that *can* be made async) would silently
  invalidate the SAFETY claim. Same pattern at `app.rs:3415-3423`.
- **Risk:** A future patch makes the call path async-first; the SAFETY
  invariant becomes false; `setenv`/`getenv` race becomes UB on glibc.
- **Remediation:** Add a `OnceLock<()>` "have we spawned threads yet"
  guard (set by the moment we launch the IPC client) and assert it is
  unset before calling `remove_var`. Or — better — read the env var
  once via `std::env::var_os` into a `SecretString` and never call
  `remove_var` at all, accepting the residual `/proc/self/environ`
  exposure as documented in `--help`.

---

### LOW-08-8 — `parse_shell` accepts `elvish` but the help-text subcommand list omits it

- **Severity:** LOW
- **File:** `crates/pcloud-cli/src/app.rs:368`,
  `crates/pcloud-cli/src/completion.rs:663-672`
- **Evidence:** `app::help_text()` documents `completion <bash|zsh|fish|elvish|powershell>`
  in the SHELL COMPLETION section (line 368) — wait, actually it says
  `<bash|zsh|fish|elvish|powershell>` correctly. Let me verify…
  `app.rs:368` reads `"    completion <bash|zsh|fish|elvish|powershell>\n"`.
  This is consistent with `parse_shell` which accepts all five. Closing
  this finding as a no-op; the audit cross-check passed.
- **Status:** No issue.

---

### LOW-08-9 — Completion test only checks "non-empty + contains pcloudc", no shape assertion

- **Severity:** LOW
- **File:** `crates/pcloud-cli/src/completion.rs:684-746`
- **Evidence:** Tests `bash_completion_non_empty`, `zsh_completion_non_empty`,
  `fish_completion_non_empty`, `elvish_completion_non_empty`,
  `powershell_completion_non_empty` only assert the output contains
  the literal string `pcloudc`. They do not check that the script is
  syntactically loadable, that all subcommands are emitted, or that
  the output round-trips through the shell.
- **Risk:** A regression that produces a malformed completion script
  (e.g. unterminated quote, missing case branch) ships green.
- **Remediation:** For each shell, exec the shell and source the script
  in CI (`bash -n script.sh`, `zsh -n script.zsh`, `fish --no-execute
  script.fish`); fail on parse error. Add a strong test that asserts
  every `Command::*` variant has at least one completion entry.

---

### LOW-08-10 — Build script silently drops `GIT_HASH` when not in a git checkout, debug builds also skip the warning

- **Severity:** LOW
- **File:** `crates/pcloud-cli/build.rs:30-53`,
  `crates/pcloud-cli/src/main.rs:55-70`
- **Evidence:** `build.rs:50-52` silently swallows non-zero `git
  rev-parse` exit. `main.rs:55-70` warns at runtime in release builds
  only. Tarball releases (no `.git/HEAD`) and CI docker builds without
  `GIT_HASH` env will stamp `unknown` and the operator never finds out
  unless they run a release build.
- **Risk:** A release artifact is produced with `pcloudc <ver> (unknown,
  release)` and operators cannot reproduce the build. Operationally
  weak for incident response.
- **Remediation:** Treat missing `GIT_HASH` in `--release` profile as a
  hard build failure unless the operator explicitly sets
  `PCLOUD_ALLOW_UNKNOWN_GIT_HASH=1` (then warn loudly). Reproducible-
  builds best practice.

---

### LOW-08-11 — `pcloud-sdk` examples don't have a CI build gate documented

- **Severity:** LOW
- **File:** `crates/pcloud-sdk/Cargo.toml:50-68`
- **Evidence:** Five `[[example]]` declarations, all build cleanly
  (verified: `cargo build -p pcloud-sdk --examples` succeeds). No
  documented CI step that runs this command. `pcloud_rev.md` §8:219
  expects "`crates/pcloud-sdk/examples/` compiles with `cargo build
  --examples`" as a release gate.
- **Remediation:** Add a `cargo build -p pcloud-sdk --examples` step
  to the existing CI workflow. Trivial.

---

## CLI command coverage table

`pcloudc` has an unusually wide subcommand surface (~100 routes including
aliases). The table below covers a representative sampling — every
subcommand in `Command` (commands.rs) was cross-checked against
`completion.rs` and `app::help_text()`. Findings: 100% of the
`Command::*` variants have an `into_request` mapping in
`commands.rs:into_request()`, exit codes flow through the unified
`ExitCode::from_response_status` path (`exit_code.rs:96-108`), and
help-text mentions every non-stub command. The drift risk is structural
(per HIGH-08-1) rather than per-command.

| Subcommand            | Help matches? | Completion entry? | Exit codes documented? |
|-----------------------|---------------|-------------------|------------------------|
| `status` / `st`       | Yes (`app.rs:97-99`) | Yes (`completion.rs:72`) | Standard |
| `health`              | Yes (`94-96`) | Yes (`73`) | Standard |
| `pending` / `p`       | Yes (`100-101`) | Yes (`74`) | Standard |
| `userinfo`            | Yes (`213-214`) | Yes (`75`) | Standard |
| `pause` / `resume`    | Yes (`260`) | Yes (`76-77`) | Standard |
| `login` (REPL)        | Yes (`106-186`) | Top-level only (`325`) — login flag inventory hand-rolled, not in completion tree | Documented in `EXIT_CODE_HELP` (3=Auth, 7=Conflict) |
| `logout`              | Yes (`188-191`) | Yes (`326`) | Standard |
| `submit-password`     | Yes (`194-200`) | Yes (`329`) | 3=Auth |
| `submit-auth`         | Yes (`202-204`) | Yes (`330`) | 3=Auth |
| `submit-tfa`          | Yes (`205`) | Yes (`331`) | 3=Auth |
| `submit-recovery`     | Yes (`207`) | Yes (`332`) | 3=Auth |
| `send-tfa-sms`        | Yes (`208`) | Yes (`327`) | 3=Auth |
| `send-tfa-notification` | Yes (`209-210`) | Yes (`328`) | 3=Auth |
| `authsave on/off`     | Yes (`192-193`) | Yes (`335`) | Standard |
| `mount` / `unmount`   | Yes (`219-227`) | Yes (`637-638`) | Standard |
| `fs status`           | Yes (`228-230`) | Yes (`321-323`) | Standard |
| `sync list`           | Yes (`235-236`) | Yes (`81`) | Standard |
| `sync add`            | Yes (`237-249`) | Yes (`82`) — but `--type` flag missing from completion tree | 7=Conflict (duplicate) |
| `sync remove`         | Yes (`250-251`) | Yes (`83`) | Standard |
| `sync change-type`    | Yes (`252-255`) | Yes (`85-101`) | Standard |
| `sync localscan`      | Yes (`256-257`) | Yes (`102-108`) | Standard |
| `sync suggest`        | Yes (mentioned tersely) | Yes (`109-116`) | Standard |
| `sync is-syncable`    | Yes | Yes (`117-127`) | Standard |
| `crypto start <PW>`   | Yes (`278-280`) | Yes (`136`) — `<PW>` arg not in completion tree | 5=CryptoLocked |
| `crypto stop`         | Yes (`281`) | Yes (`137`) | Standard |
| `crypto status`       | Yes (`282`) | Yes (`138`) | Standard |
| `crypto reset`        | Yes (mentioned) | Yes (`139-142`) | Standard |
| `crypto setup`        | Yes (`283-289`) | Yes (`172-203`) | Standard |
| `crypto change-password` | Yes (mentioned) | Yes (`152-158`) | 3=Auth, 5=CryptoLocked |
| `crypto get-folder-key` | Yes (`290-291`) | Yes (`205-246`) — full security flag set | Standard |
| `crypto get-file-key` | Yes (`292-293`) | Yes (`247-289`) | Standard |
| `notifications list`  | Yes (`349`) | Yes (`294`) | Standard |
| `notifications mark-read` | Yes (`350-351`) | Yes (`295`) | Standard |
| `audit verify`        | Yes (`357-363`) | Yes (`303-306`) | Standard |
| `publink send`        | Yes (`323-325`) | Yes (`309-311`) | Standard |
| `folder create/id/flags/owner` | Yes (`265-269`) | Yes (`313-318`) | Standard |
| `stat <PATH>`         | Yes (`270-273`) | Yes (`382`) | Standard |
| `reload` (SIGHUP)     | Yes (`91-93`) | Yes (`383`) | Standard |
| `drain`               | Yes (mentioned) | Yes (`385-387`) | Standard |
| `slo`                 | Yes | Yes (`389-391`) | Standard |
| `integrity status/run-once/skip` | Yes | Yes (`392-401`) | Standard |
| `ha status`           | (not in help-text) | Yes (`402-406`) | Standard |
| `audit-verifier status` | (not in help-text) | Yes (`407-411`) | Standard |
| `upload create/pause/resume/cancel/list` | (not in help-text) | Yes (`413-426`) | Standard |
| `conflict list/resolve` | (not in help-text) | Yes (`427-438`) | Standard |
| `snapshot create/restore/verify/prune` | (not in help-text) | Yes (`439-461`) | Standard |
| `verify <PATH>`       | (not in help-text in detail) | Yes (`463-478`) | Standard |
| `migrate-from-c`      | (not in help-text) | Yes (`479`) | Standard |
| `backup create/delete/stop-device/delete-device/snapshot-*` | (not in help-text) | Yes (`481-493`) | Standard |
| `download link/file`  | (not in help-text) | Yes (`495-519`) | Standard |
| `account verify-email/verify-email-restricted/lost-password/change-password/register/api-servers/set-api-server/set-language/promo` | (not in help-text) | Yes (`520-581`) | Standard |
| `log <PATH>` (revisions) | Yes (mentioned in app.rs handler) | Yes (`583-598`) | Standard |
| `diff` / `restore`    | Documented as stubs in handler | Hidden in completion (`599-636`) | 6=Unavailable always |
| `start` (spawn pcloudd) | Yes (`75-85`) | Yes (`639`) | Standard |
| `finalize` / `stop` / `f` | Yes (`86-90`) | Yes (`640`) | Standard |
| `doctor`              | (not in help-text) | Yes (`641`) | 6=Unavailable on failed checks |
| `completion <shell>`  | Yes (`368-374`) | Yes (`643-651`) | Standard |
| `--version`           | Yes (`46-47`) | Builds banner with pkg-version + git-hash + profile (`main.rs:81-91`) | n/a |

**Coverage gaps (non-blocking, MEDIUM at most):** The hand-rolled
help-text in `app.rs` does not document several recent subcommands
(`ha`, `audit-verifier`, `upload`, `conflict`, `snapshot`, `verify`,
`migrate-from-c`, `backup`, `download`, `account`, `doctor`). They are
all in the completion tree and have `Command::*` variants with proper
IPC mappings — the operator-facing manual page just hasn't been kept
in sync. Track as a docs hygiene follow-up.

---

## SDK public-surface review

### Public re-exports (semver discipline)

- **`pcloud_sdk::Notification = pcloud_proto::Notification`** —
  `lib.rs:113-117`. Type alias; couples SDK semver to proto crate. See
  MEDIUM-08-4.
- **`pcloud_sdk::{ConflictMode, DEFAULT_CHUNK_SIZE, FileMetadata,
  UploadConfig, UploadError, UploadHandle, UploadPayload,
  UploadProgress, UploadRequest, UploadSession, UploadSessionDriver,
  UploadState}`** — `lib.rs:108-111`. All defined inside
  `pcloud_sdk::upload_session`, so they are SDK-owned. Compliant.
- **`pcloud_sdk::CRATE_NAME`** — `lib.rs:101-105`. SDK-owned constant.
- All 14 helper error enums (`UploadHelperError`, `BackupHelperError`,
  `AccountUtilityError`, `NotificationsHelperError`,
  `FolderMetadataError`, `FileMutationHelperError`, `MountHelperError`,
  `PublinkHelperError`, `TreePublicLinkHelperError`, `CryptoHelperError`,
  `AuthHelperError`, `DownloadHelperError`, `CreateFolderHelperError`,
  `SettingKvError`, `ValueKvError`) and the unified `SdkError` are
  SDK-owned. Compliant.
- The crate uses `pcloud_config::{ConfigProfile, Environment}` and
  `pcloud_secret::{SecretString, ExposeSecret}` internally (private).
  Both are exposed via method signatures (`config() -> &ConfigProfile`,
  `auth_token_secret() -> Option<SecretString>`) — this is `pub fn`
  signature coupling, not `pub use`. The §Semver section in `lib.rs:52-61`
  explicitly documents the rule and acknowledges this is fine because
  callers can add a direct dep. Compliant given the documentation.

### Doc comments on public functions

`#![deny(missing_docs)]` is enforced (`lib.rs:78`). `cargo doc -p
pcloud-sdk --no-deps` emits zero warnings on the public surface
(`RUSTDOCFLAGS="-D missing-docs"` confirmed). The two warnings under
`--document-private-items` are the broken intra-doc links in
`upload_session.rs` (MEDIUM-08-3). Spot-checked public methods at
`lib.rs:1329` (`runtime_summary`), `:1342` (`config`), `:1376`
(`dispatch`), `:1735` (`get_api_servers`), `:1791` (`set_language`),
`:1948` (`set_api_server`), `:2961` (`userinfo`), `:3145`
(`download_file`) — all have full rustdoc with examples (`# use
pcloud_sdk::EmbeddedDaemon;` runnable doctest blocks gated `no_run`).

### Example compilation status

```
cargo build -p pcloud-sdk --examples
... Finished `dev` profile
```
All five examples compile clean: `login_and_list`, `upload_and_download`,
`public_link`, `crypto_lifecycle`, `create_tree_public_link_from_paths`.
No warnings.

### SDK happy-path test coverage

- **Unit tests in `lib.rs`:** 46 tests, covering: `EmbeddedDaemon`
  dispatch, plugin registration policy, upload helpers (data, file,
  data_as, file_as, missing-folder rejection), account utilities, auth
  helpers (authenticated + unauthenticated paths), TFA flow, download
  helpers, notifications, folder metadata, filesystem status, stat,
  list_folder, mount preconditions, error category mapping. Coverage is
  broad enough that `pcloud_rev.md` §8:220 ("SDK tests cover the happy
  path for each helper") is satisfied for the in-process surface.
- **Integration tests in `tests/`:** 4 tests in
  `upload_session_chunked.rs` covering the chunked upload state machine
  with a `MockDriver` (no live pCloud).
- **Live E2E tests:** None in `pcloud-sdk/tests/`. Live coverage lives
  in `crates/pcloud-live-e2e/` and is gated `PCLOUD_LIVE_E2E=1` per
  CLAUDE.md §"Live verification".

### Feature flag combinations

Declared features: `default = []` only. There are no `tls-rustls` /
`tls-native` / other gates. See MEDIUM-08-2 for the missing-flag
finding. `cargo build -p pcloud-sdk --no-default-features` succeeds
(verified during audit).

---

## What is working well (kudos)

- **Exit codes** (`exit_code.rs:1-212`) — formal stable-ABI guarantee
  documented in the module-level rustdoc, complete unit-test mapping
  from `ResponseStatus` → numeric, transport-error classifier with
  explicit substring rules. Better than most enterprise CLIs.
- **Prompt termios RAII guard** (`prompt.rs:198-213`) — survives panics
  via `Drop`. Properly cited unsafety. `--password-stdin` /
  `--password-env` paths documented end-to-end in help text.
- **`pcloudc --version`** (`main.rs:81-91`) — exactly the format the
  master prompt asked for: `pcloudc <pkg-version> (<git-hash>,
  <profile>)`. Soft-failure cascade in `build.rs` is sensible.
- **Help text** (`app.rs:16-466`) — 451 lines of hand-curated man-page-
  style documentation including config file keys, env vars, exit-code
  reference, and worked examples. Higher quality than most systemd-style
  daemons.
- **SDK semver section** (`lib.rs:52-61`) — explicit `#§8:221 audit
  compliance` cite, sets the rule, names the only known violation
  (`Notification` alias) in line.
- **SDK `forbid(unsafe_code)`** (`lib.rs:1`) — combined with
  `deny(missing_docs)` (`lib.rs:78`), the SDK is the strictest crate in
  the workspace by lint posture.

---

## Cross-references

- `pcloud_rev.md` §8 (audit scope, line 206-222)
- `CLAUDE.md` §"Security and Enterprise Rules" (secret hygiene, IPC
  posture)
- `CLAUDE.md` "Final Rule" (parity claims must match reality)
- `crates/pcloud-cli/Cargo.toml:9-13`, `crates/pcloud-sdk/Cargo.toml:9-13`
- `STATUS.md` (open Partial rows; relevant for diff/restore stubs)

---

## Severity rollup

| Severity | Count |
|----------|-------|
| CRITICAL | 0 |
| HIGH     | 1 |
| MEDIUM   | 5 |
| LOW      | 4 |
| Total    | 10 (one LOW closed as no-op after verification) |

End of audit 08.

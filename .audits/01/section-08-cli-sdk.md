## Section 8. CLI & SDK Surface

**Auditor:** Dimension 8 specialist (CLI / SDK ergonomics)
**Scope:** `crates/pcloud-cli/` end-to-end, `crates/pcloud-sdk/` public API, `pcloud-ipc::{Method,Request}` ↔ CLI mapping, shell-completion coverage, feature-flag matrix.
**Non-scope:** parity with the legacy C tree (Dimension 1), daemon-side handler correctness, protocol wire behaviour. Findings here are about UX, semver hygiene, argument-parser truthfulness, and operational ergonomics only.
**Workspace root:** `/home/ezechiel203/Projects/FORKS/pcloud-rs/`

---

### 8.0 Executive summary

`pcloudc` is the only installable CLI binary. It is **hand-rolled** — not clap-derive — despite `clap` being declared as a dependency. Every subcommand is dispatched through a bespoke token walker in `app.rs` (`normalize_args` + `parse_inputs_for_command`, ~4700 lines). Clap is used only to *generate* shell-completion scripts from a **second, parallel, smaller** command tree (`completion::build_cli`). Those two trees are **not mechanically synchronised**, and the audit found that completion is missing roughly **two-thirds of the subcommands** the CLI actually accepts. That is the single biggest CLI UX finding in this section.

The SDK is a much tidier surface: one `EmbeddedDaemon` struct, 14 typed per-helper error enums all funnelled into `SdkError`, docstrings on virtually every public item, and a curated re-export block (no wildcard glob leaks). It still has two SemVer-discipline issues and **only one example** (vs. the 80+ helper methods advertised).

Severity tallies:

| Severity | CLI | SDK | Cross | Total |
|---|---|---|---|---|
| CRITICAL | 0   | 0   | 0   | 0   |
| HIGH     | 6   | 3   | 2   | 11  |
| MEDIUM   | 11  | 6   | 3   | 20  |
| LOW      | 8   | 4   | 2   | 14  |

No CRITICAL findings — the CLI *works*, secrets do not trivially leak to stdout or history, exit codes are stable, and the SDK compiles cleanly with the declared feature set. The HIGHs are operational-quality defects (shell completion drift, accidental `pub use`, single example, one-arg `clap` that doesn't reflect the real surface).

---

### 8.1 CLI command ↔ IPC Request / Method matrix

Source of truth:
- IPC: `crates/pcloud-ipc/src/methods.rs:37` (`enum Method`, 42 variants) and `crates/pcloud-ipc/src/methods.rs:262` (`enum Request`, 70+ variants).
- CLI: `crates/pcloud-cli/src/commands.rs:35` (`enum Command`, 113 variants) and `crates/pcloud-cli/src/commands.rs:750` (`into_request`).
- Completion tree: `crates/pcloud-cli/src/completion.rs:35` (`build_cli`, ~55 subcommand entries).

Legend: **C** = visible in CLI, **R** = visible through `into_request`, **K** = in `clap_complete` tree, **—** = absent.

#### 8.1.1 Argumentless `Method::*` variants (wired via `Request::Plain { method }`)

| `Method` variant | CLI subcommand | `into_request`? | Completion? | Notes |
|---|---|---|---|---|
| `GetStatus` | `status` / `st` | yes (`commands.rs:757`) | yes | |
| `GetHealth` | `health` | yes (`:760`) | yes | Also used as defensive fallback for CLI-side-only commands (`Start`, `Drain`, `Reload`, `Doctor`, `MigrateFromC`, `FileDiff`, `FileRestore`) — see Finding CLI-M3. |
| `Health` (enterprise) | — **missing** | — | — | HIGH: `Method::Health` has no CLI subcommand. Operators cannot query the structured `/healthz` payload (build info, uptime, Prometheus snapshot) without `dispatch()` via SDK. Finding **CLI-H1**. |
| `GetPending` | `pending` / `p` | yes (`:763`) | yes | |
| `GetSyncRoots` | `sync list` / `sync ls` / `sync-list` | yes (`:865`) | partial (`sync list` in completion) | |
| `ListPublicLinks` | `list-links` / `publink list` | yes (`:769`) | `list-links` only in completion | MEDIUM: no `publink` subcommand tree under completion, only the hyphenated form. Finding **CLI-M2**. |
| `ListUploadLinks` | `list-upload-links` | yes (`:779`) | yes | |
| `GetUserInfo` | `userinfo` | yes (`:886`) | yes | |
| `PauseSync` | `pause` | yes (`:889`) | yes | |
| `ResumeSync` | `resume` | yes (`:892`) | yes | |
| `LoginBegin` | `login` (REPL) / `login-begin` | yes (`:895`) | yes (`login`) | |
| `Logout` | `logout` | yes (`:898`) | yes | |
| `SendTwoFactorSms` | `send-tfa-sms` | yes (`:901`) | yes | |
| `SendTwoFactorNotification` | `send-tfa-notification` | yes (`:904`) | yes | |
| `SubmitPassword` | via REPL; `submit-password` (legacy) | `PasswordSubmission` (`:907`) | yes | |
| `SubmitTwoFactorCode` | `submit-tfa` | `TwoFactorCodeSubmission` (`:917`) | yes | |
| `UnlockCrypto` | `unlock-crypto`, `crypto start` | `CryptoUnlock` (`:924`) | yes | |
| `LockCrypto` | `lock-crypto`, `crypto stop` | yes (`:930`) | yes | |
| `GetCryptoStatus` | `crypto status` / `crypto st` | yes (`:776`) | partial (only `start` / `stop` in completion) | HIGH — see **CLI-H2**. |
| `CryptoReset` | `crypto reset` | yes (`:1175`) | — | |
| `GetCryptoPrivKeyFlags` | `crypto priv-key-flags` | yes (`:1178`) | — | |
| `SendCryptoChangeUserPrivate` | `crypto send-change-private` | yes (`:1181`) | — | |
| `GetCryptoHint` | `crypto hint` | yes (`:1197`) | — | |
| `Shutdown` | `shutdown` / `stop` / `finalize` / `f` | yes (`:939`) | yes (`finalize`) | MEDIUM: `shutdown` token itself isn't in the completion tree — see **CLI-M2**. |
| `SetAuthPersistence` | `authsave` | `AuthPersistence` (`:927`) | yes | |
| `ListIncomingShares` | `list-incoming-shares` / `shares list-incoming` | yes (`:956`) | yes (`list-incoming-shares`) | |
| `ListOutgoingShares` | `list-outgoing-shares` / `shares list-outgoing` | yes (`:959`) | yes | |
| `ListIncomingShareRequests` | `list-incoming-share-requests` | yes (`:962`) | yes | |
| `ListOutgoingShareRequests` | `list-outgoing-share-requests` | yes (`:965`) | yes | |
| `ListContacts` | `list-contacts` / `contacts list` | yes (`:968`) | yes (`list-contacts`) | |
| `ListMyTeams` | `list-myteams` / `teams list` | yes (`:971`) | yes (`list-myteams`) | |
| `ListNotifications` | `notifications list`, `notif list`, `list-notifications` | yes (`:772`) | yes | |
| `SessionStatus` | `session status` | yes (`:1022`) | yes | |
| `FileHistory` | `log` / `history` (plus typed-args variant) | `Request::FileHistory` (`:1085`) | — | HIGH: `log` is documented in `help_text` (line 357+) but absent from completion. Finding **CLI-H3**. |
| `IntegrityStatus` | `integrity status` / `integrity` | yes (`:1139`) | — | |
| `HaStatus` | `ha status` / `ha` | yes (`:1146`) | — | |
| `DrainStatus` | `drain` (CLI-side only, `GetHealth` fallback) | `Plain { method: DrainStatus }` (`:947`) | — | |
| `GetSlo` | `slo` | yes (`:766`) | — | HIGH: `slo` is advertised in help & referenced in field-selector tests but missing from completion. |
| `GetAuditVerifierStatus` | `audit-verifier status` | yes (`:1149`) | — | |
| `GetSyncStatus` | `sync status` / `sync st` | yes (`:868`) | — | |
| `ListConflicts` | `conflict list` / `conflicts` | yes (`:1169` / alias in app.rs sync subcmd) | — | |
| `StatPath` | `stat` | `Request::StatPath` (`:1071`) | — | |
| `GetApiServers` | `account api-servers` / `account apiservers` | yes (`:1231`) | — | |
| `GetPromo` | `account promo` | yes (`:1241`) | — | |
| `VerifyEmail` | `account verify-email` | yes (`:1209`) | — | |

#### 8.1.2 Argument-bearing `Request::*` variants (not carried by `Method` alone)

| `Request` variant | CLI subcommand | `into_request`? | Completion? | Notes |
|---|---|---|---|---|
| `PasswordSubmission` | `submit-password`, REPL login | yes (`:907`) | yes | |
| `AuthTokenSubmission` | `submit-auth` | yes (`:914`) | yes | |
| `TwoFactorCodeSubmission` | `submit-tfa`, `submit-recovery` | yes (`:917`) | yes | |
| `CryptoUnlock` | `unlock-crypto` | yes (`:924`) | yes | |
| `CryptoSetup` | — **MISSING from CLI** | — | — | HIGH: no `crypto setup` subcommand. `Request::CryptoSetup { password, hint }` is wired at the daemon side (`methods.rs:304`) but unreachable from `pcloudc`. Finding **CLI-H4**. |
| `CryptoMkdir` | — **MISSING** | — | — | HIGH: no `crypto mkdir` subcommand. A user cannot create an encrypted folder from the CLI. Finding **CLI-H4**. |
| `CryptoChangePassword` | `crypto change-password` | yes (`:1184`) | — | |
| `CryptoChangePasswordUnlocked` | `crypto change-password-unlocked` | yes (`:1191`) | — | |
| `AuthPersistence` | `authsave <on\|off>` | yes (`:927`) | yes | |
| `SyncRootAdd` | `sync add` | yes (`:871`) | yes | |
| `SyncRootRemove` | `sync remove` | yes (`:876`) | yes | |
| `SyncRootPause` | — **MISSING** | — | — | MEDIUM: `Request::SyncRootPause { sync_id }` and `SyncRootResume` exist (methods.rs:396, 401) but the CLI has no `sync pause-root <id>` / `sync resume-root <id>` — only the global `pause` / `resume` which trigger `PauseSync` / `ResumeSync` on the whole daemon. Finding **CLI-M1**. |
| `SyncRootResume` | — **MISSING** | — | — | See CLI-M1. |
| `SyncRootChangeType` | `sync change-type` | yes (`:879`) | — | |
| `GetSyncSuggestions` | `sync suggest` | yes (`:1201`) | — | |
| `IsFolderSyncable` | `sync is-syncable` | yes (`:1205`) | — | |
| `ShowPublicLink` | `show-link` / `publink show` | yes (`:782`) | yes | |
| `DeletePublicLink` / `DeletePublicLinkByCode` | `delete-link` | yes (`:785`+) | yes | |
| `CreateFilePublicLink` | `create-file-link` | yes (`:800`) | yes | |
| `CreateFolderPublicLink` | `create-folder-link` | yes (`:803`) | yes | |
| `ChangePublicLinkExpire` | `change-link-expire` | yes (`:806`) | yes | |
| `ChangePublicLinkPassword` | `change-link-password` | yes (`:810`) | yes | |
| `ChangePublicLinkUpload` | `change-link-upload` | yes (`:820`) | yes | |
| `CreateUploadLink` | `create-upload-link` | yes (`:824`) | yes | |
| `DeleteUploadLink` | `delete-upload-link` | yes (`:831`) | yes | |
| `CreateTreePublicLink` | `create-tree-link` | yes (`:834`) | yes | |
| `ListPublicLinkAccess` | `list-link-access` | yes (`:843`) | yes | |
| `AddPublicLinkAccess` | `add-link-access` | yes (`:846`) | yes | |
| `RemovePublicLinkAccess` | `remove-link-access` | yes (`:850`) | yes | |
| `ListBookmarks` | `list-bookmarks` / `bookmark list` | yes (`:854`) | yes | |
| `RemoveBookmark` | `remove-bookmark` / `bookmark remove` | yes (`:855`) | yes | |
| `ChangeBookmark` | `change-bookmark` / `bookmark change` | yes (`:859`) | yes | |
| `ShareFolder` | `share-folder` | yes (`:974`) | yes | |
| `CancelShareRequest` | `cancel-share-request` | yes (`:982`) | yes | |
| `DeclineShareRequest` | `decline-share-request` | yes (`:985`) | yes | |
| `AcceptShareRequest` | `accept-share-request` | yes (`:988`) | yes | |
| `RemoveShare` | `remove-share` | yes (`:993`) | yes | |
| `ModifyShare` | `modify-share` | yes (`:996`) | yes | |
| `AccountStopShare` | `account-stopshare` | yes (`:1000`) | yes | |
| `AccountModifyShare` | `account-modifyshare` | yes (`:1004`) | yes | |
| `AccountTeamShare` | `account-teamshare` | yes (`:1008`) | yes | |
| `ValueGet` / `ValueSet` / `ValueHas` | — **MISSING from CLI** | — | — | MEDIUM: the typed KV helpers `Request::ValueGet/Set/Has` (methods.rs:635+) are exposed through the SDK (`EmbeddedDaemon::{get,set,has}_{uint,int,bool,string}_value`) but there is **no `pcloudc value get/set/has`** subcommand. Operators cannot probe or adjust settings without writing a Rust host. Finding **CLI-M4**. |
| `MarkNotificationsRead` | `notifications mark-read <id>` | yes (`:775`) | yes | |
| `AuditVerifyChain` | `audit verify` | yes (`:1016`) | yes | |
| `Mount` / `Unmount` / `MountForceUnmount` | `mount` / `unmount` (+ `PCLOUD_FORCE_UMOUNT`) | yes (`:1025`,1028,1038) | yes (`mount`,`unmount`) | |
| `CreateRemoteFolder` | `folder create` | yes (`:1053`) | yes | |
| `Unmount` | `unmount` | yes (`:1044`) | yes | |
| `RunLocalScan` | `sync localscan` | yes (`:1047`) | — | |
| `SendPublink` | `publink send` | yes (`:1048`) | yes | |
| `GetFolderIdByPath` / `GetFolderFlags` / `GetFolderOwnerId` | `folder id` / `folder flags` / `folder owner` | yes (`:1059`,1062,1065) | yes | |
| `FilesystemStatus` | `fs status` | yes (`:1068`) | yes | |
| `StatPath` | `stat` | yes (`:1071`) | — | |
| `FileHistory` | `log` / `history` | yes (`:1085`) | — | |
| `VerifyPath` | `verify` | yes (`:1102`) | — | CLI-side walker is primary; IPC is a future wiring. |
| `BackupSnapshot` (Create/Restore/Verify/Prune) | `snapshot create/restore/verify/prune` + deprecated `backup snapshot-*` | yes (`:1106` etc.) | — | |
| `IntegrityRunOnce` / `IntegritySkip` | `integrity run-once` / `integrity skip` | yes (`:1142`,1143) | — | |
| `UploadCreate` / `UploadPause` / `UploadResume` / `UploadCancel` / `UploadList` | `upload create/pause/resume/cancel/list` | yes (`:1152`→1168) | — | HIGH — entire `upload` command group absent from completion. Finding **CLI-H3**. |
| `ConflictList` / `ConflictResolve` | `conflict list` / `conflict resolve` | yes (`:1169`,1170) | — | |
| `LostPassword` | `account lost-password` | yes (`:1215`) | — | |
| `VerifyEmailRestricted` | `account verify-email-restricted` | yes (`:1212`) | — | |
| `AccountChangePassword` | `account change-password` | yes (`:1218`) | — | |
| `AccountRegister` | `account register` | yes (`:1226`) | — | |
| `SetApiServer` | `account set-api-server` | yes (`:1234`) | — | |
| `SetLanguage` | `account set-language` | yes (`:1238`) | — | |
| `GetFileLink` | `download link` | yes (`:1245`) | — | |
| `DownloadFile` | `download file` | yes (`:1248`) | — | |
| `DeleteBackup` | `backup delete` / `backup rm` | yes (`:1253`) | — | |

**Matrix aggregate:**
- IPC `Method` variants: 42. CLI-reachable: 41. Completion-listed (top-level): 16/42 (≈38%). Unreachable: 1 (`Health` — CLI-H1).
- IPC `Request` variants (non-Plain): 61. CLI-reachable via `Command::into_request`: 58. Unreachable: **3** (`CryptoSetup`, `CryptoMkdir`, `SyncRootPause`, `SyncRootResume`, `ValueGet`/`Set`/`Has`) — see CLI-H4, CLI-M1, CLI-M4.
- CLI `Command` variants with no completion stub: **~50 of 113** (~44%). Account/download/backup/snapshot/integrity/ha/audit-verifier/conflict/upload/stat/log/session/folder trees are all absent or partial. This is the single largest CLI UX defect and maps to **CLI-H3**.

---

### 8.2 CLI findings

#### 8.2.1 Argument parser truthfulness

**CLI-H1 — `Method::Health` (enterprise health probe) is unreachable from the CLI.**
- Evidence: `pcloud-ipc/src/methods.rs:44-49` defines `GetHealth` *and* `Health` as two distinct methods (the latter reportedly returns build info / uptime / Prometheus snapshot). `pcloud-cli/src/commands.rs:47-51` only maps `pcloudc health` → `Method::GetHealth`. Grepping `commands.rs` + `main.rs` finds **no** reference to `Method::Health`.
- Impact: operators running `pcloudc health --json` get the short liveness string, not the structured payload documented on the IPC variant. The `--json` promise (machine-readable health) is broken.
- Remediation: either (a) add `pcloudc health --detailed` / `pcloudc healthz` → `Method::Health`, or (b) switch `pcloudc health` to `Method::Health` and rename the current short path to `pcloudc ping`. Update `help_text()` (`app.rs:95`) and `completion.rs:73`.
- Severity: HIGH.

**CLI-H2 — Shell-completion tree is desynchronised from the real command surface.**
- Evidence: `crates/pcloud-cli/src/completion.rs:35-193` hand-maintains a parallel clap tree. Spot-check against `commands.rs:35-553`:
  - missing top-level subcommands: `account`, `backup`, `snapshot`, `conflict`, `integrity`, `ha`, `audit-verifier`, `upload`, `download`, `stat`, `slo`, `verify`, `log`, `diff`, `restore`, `ha`, `drain`, `reload`, `doctor`, `migrate-from-c`, `teams`, `contacts`.
  - missing subtrees under declared groups: `crypto` completion (`completion.rs:86-90`) only carries `start` / `stop` — absent `status`, `reset`, `hint`, `priv-key-flags`, `send-change-private`, `change-password`, `change-password-unlocked`.
  - missing `sync` children: `status`, `change-type`, `localscan`, `conflicts`, `suggest`, `is-syncable`.
  - missing global flags: `--field` / `-f` / `--select`, `--dbg` / `--debug`, `--trace-id`, `--config`.
- Impact: users who install completion via `pcloudc completion bash > /etc/bash_completion.d/pcloudc` get tab-completion that silently excludes ≈60% of the real surface. This is the strongest CLI UX regression found. The tests in `completion.rs:225-284` only verify that scripts are *non-empty*, not that they list the real commands.
- Remediation: either migrate the whole CLI to `clap::Parser`-derive (single source of truth, which the `Cargo.toml` already enables with `features = ["derive"]` at line 27 but never uses), or write an integration test that enumerates every `Command::*` variant and asserts it exists in the clap completion tree.
- Severity: HIGH.

**CLI-H3 — `--help` promises commands that completion never advertises.**
- Evidence: `app.rs:300` lists `publink send`, lines 324-327 list `notifications mark-read`, lines 332-337 list `audit verify`, line 419 lists `status auth sync crypto`, etc. Of these, only `publink send` and the notifications pair appear in `build_cli`.
- Impact: help text accurate, completion broken. Same remediation as CLI-H2 but called out separately because *scripts* that `grep` the help output will find commands that shell completion won't assist with.
- Severity: HIGH.

**CLI-H4 — `crypto setup` and `crypto mkdir` are unreachable from `pcloudc`.**
- Evidence: `pcloud-ipc/src/methods.rs:304-322` wires `Request::CryptoSetup { password, hint }` and `Request::CryptoMkdir { name, parent_folder_id, local_folder_id }`. `commands.rs` has **no** `Command::*` variant that translates to either request. The `crypto start` subcommand (`app.rs:632`) only dispatches `SubmitCryptoPassword` → `CryptoUnlock`, so a first-time user cannot set up a crypto shell without embedding the SDK.
- Impact: a legitimate C-parity feature (first-run crypto bring-up) is CLI-inaccessible.
- Remediation: add `pcloudc crypto setup` (prompts new passphrase + optional hint) and `pcloudc crypto mkdir <name> [--parent <id>]`. Wire into the completion tree simultaneously.
- Severity: HIGH.

**CLI-H5 — No clap-derive anywhere: duplicate truth, massive surface for drift.**
- Evidence: `Cargo.toml:27` declares `clap = { version = "4.6", ..., features = ["std", "help", "usage", "error-context", "derive"] }`. The `derive` feature is enabled. Yet `grep -rn "#\[derive(Parser)\]\|Parser::parse\|clap::Parser" crates/pcloud-cli/src/` returns zero matches. Clap is used **only** inside `completion::build_cli` as an `Arg::new(...)` builder. The real parser is 4700 lines of hand-rolled token walking in `app.rs`.
- Impact: every subcommand is declared twice (`Command` enum + parser table) and in a third place for completion (`build_cli`). Invariants between help text, completion, and actual parser behaviour are maintained by convention only. This is the structural cause of CLI-H2 and CLI-H3.
- Remediation: migrate to `#[derive(clap::Parser)]` — or at minimum write a trivariate test (`Command::iter()` must map onto both `build_cli().get_subcommands()` and every `app::parse_command` token).
- Severity: HIGH.

**CLI-H6 — Positional password on `pcloudc crypto start <PW>` and `pcloudc submit-password [USER] [PW]` leaks into argv, then into `/proc/<pid>/cmdline`, `ps auxw`, and shell history.**
- Evidence:
  - `app.rs:1545`: `let crypto_password = match args.get(2) { Some(password) => password.clone(), None => SecretPrompt::new("Crypto password").read_secret()? };` — a positional third argument *is* consumed if present, no warning printed.
  - `app.rs:2997-3014`: `submit-password` with a positional password prints a one-line stderr warning (`"warning: passing the password on the command line is insecure ..."`) but still accepts the value. The best-effort argv scrub comment concedes "we can't mutate \[`&[String]`\]" and `/proc/self/cmdline` "is a separate kernel-maintained copy that we cannot rewrite".
  - `app.rs:1527-1533` (`submit-auth`): accepts positional token with **no** stderr warning. The token is equally sensitive (long-lived bearer credential).
- Impact: any same-user process on the host can read the full password via `/proc/<pid>/cmdline` until the CLI exits, and the value survives in `~/.bash_history` / `~/.zsh_history`. The scrub-argv comment is candid but the feature still exists.
- Remediation:
  - Deprecate positional password/token on all three subcommands (`submit-password`, `submit-auth`, `crypto start`). Print a deprecation warning in this release; reject in the next major.
  - Require `--password-stdin` / `--password-env` / interactive prompt (already implemented for the REPL path).
  - At minimum, add the same stderr warning to `submit-auth` and `crypto start <PW>` that `submit-password` has.
- Severity: HIGH.

#### 8.2.2 Exit codes & error mapping

**CLI-M1 — `SyncRootPause` / `SyncRootResume` (per-root) have no CLI surface.**
- Evidence: `pcloud-ipc/src/methods.rs:396-404` wires both. `pcloudc sync pause` / `sync resume` route to **daemon-wide** `Pause`/`Resume` (`app.rs:554-555`). There is no way to pause a single root via the CLI.
- Impact: operators who want to pause one sync root without stopping the whole engine must use the SDK directly.
- Remediation: add `pcloudc sync pause-root <id>` / `sync resume-root <id>`, add to completion tree.
- Severity: MEDIUM.

**CLI-M2 — Completion tree advertises tokens not in the hand-rolled parser.**
- Evidence: `completion.rs:180-182` declares `start` (→ background daemon), `finalize` (alias for `stop`), `doctor`. Of these, `finalize` works, `start` works, `doctor` works. But `completion.rs:181` declares a literal `"finalize"` with help text `"Stop daemon and exit"` — this is the operator-facing token. The hand-rolled parser also accepts `stop` and `f`, but neither appears in completion.
- Impact: tab-completion shows only one of three synonymous tokens; users who learned `stop` from `help` cannot tab-complete it.
- Remediation: consolidate on clap-derive (CLI-H5) or add each alias explicitly.
- Severity: MEDIUM.

**CLI-M3 — Seven CLI-side-only commands all map to `Method::GetHealth` as a "defensive fallback" in `Command::into_request`.**
- Evidence: `commands.rs:754-756, 933-938, 947-955, 1077-1084, 1092-1094`. `Help`, `Start`, `Drain`, `Reload`, `Doctor`, `MigrateFromC`, `FileDiff`, `FileRestore` all fall through to `Method::GetHealth`. The comment rightly notes they "must never reach this dispatch", but if `main.rs` regresses, a `pcloudc start` would silently ping `GetHealth` and report OK.
- Impact: a bug in `main.rs` branching would silently do the wrong thing (mask failure as success).
- Remediation: make `into_request` return `Option<Request>` (or a dedicated `NoIpcDispatch` variant) and unify the CLI-side-only branch through a single "never lower to IPC" path. Tests should assert that the CLI-side-only commands never reach the IPC client.
- Severity: MEDIUM.

**CLI-M4 — `value`/`setting` CLI subcommands missing; gated behind SDK only.**
- Evidence: `pcloud-ipc/src/methods.rs:635-659` and `pcloud-sdk/src/lib.rs:2549-2696` expose typed KV helpers. The CLI has no `pcloudc value get <name> --kind=bool`.
- Impact: operators troubleshooting settings must write a Rust host.
- Remediation: add a `pcloudc value` / `pcloudc setting` subcommand tree.
- Severity: MEDIUM.

**CLI-M5 — `ExitCode::classify_transport_error` uses substring matching on localised-free English tokens.**
- Evidence: `exit_code.rs:117-137`. Matches `"connection"`, `"timed out"`, `"refused"`, `"broken pipe"`, `"unauthorized"`, `"crypto"`, etc.
- Impact: if any transport layer ever returns a localised error (or wraps error strings), the classifier silently degrades to `GenericError`. Also: `l.contains("auth") && l.contains("fail")` has classic precedence footgun — `"unauthorized" || "auth" && "fail"` parses as `"unauthorized" || ("auth" && "fail")`, which is what the author intended but is fragile (`clippy::needless_bool`-adjacent).
- Remediation: use typed errors at the transport layer (`thiserror`-enumerated IPC client errors) and match on variants, not substrings.
- Severity: MEDIUM.

**CLI-M6 — `PolicyViolation` response status forces exit code 7 (`Conflict`), which is the same code as "duplicate sync root".**
- Evidence: `exit_code.rs:105` — `ResponseStatus::PolicyViolation { .. } => Self::Conflict`.
- Impact: operators cannot distinguish a data-residency policy rejection from a state-conflict without parsing the response message. Scripts that branch on exit code lose information.
- Remediation: add `ExitCode::PolicyRejected = 9` (additive, SemVer-safe per the crate's stated "additive" guarantee at `exit_code.rs:52`). Update `EXIT_CODE_HELP`.
- Severity: MEDIUM.

**CLI-L1 — Exit codes in help text are **duplicated** across `app.rs:391-402` and `exit_code.rs:25-35` (`EXIT_CODE_HELP`).**
- Evidence: two hard-coded blocks carrying the same 0-8 list. Drift risk if either is updated.
- Impact: minor; no current discrepancy.
- Remediation: `app.rs:391-402` should `include!(EXIT_CODE_HELP)` or concat.
- Severity: LOW.

#### 8.2.3 Secrets handling on the CLI

**CLI-M7 — `submit-auth [TOKEN]` accepts the token positional with no stderr warning.**
- Evidence: `app.rs:1527-1533`. Contrast with `submit-password` positional (`app.rs:2997-3014`) which at least prints a stderr warning.
- Impact: auth tokens are bearer credentials (pCloud session token) and equally sensitive to passwords. Positional leak to `/proc/<pid>/cmdline` + shell history.
- Remediation: add the same stderr warning (and the same deprecation plan as CLI-H6) to `submit-auth`.
- Severity: MEDIUM.

**CLI-L2 — `read_masked` is Linux-only (`prompt.rs:146-246`); non-Linux falls through to `rpassword::read_password()` with no `*`-echo.**
- Evidence: `prompt.rs:154-158, 248-251`.
- Impact: on macOS/FreeBSD the 2FA prompt has no visual feedback; on Windows it uses `rpassword`'s platform default. Non-critical but documented honestly.
- Remediation: either gate the 2FA call sites to only use `read_masked` on Linux explicitly, or port the termios block to `crossterm` for portable raw-mode.
- Severity: LOW.

**CLI-L3 — `version_banner()` reports `"unknown"` for `GIT_HASH` when the build is not from a git checkout.**
- Evidence: `main.rs:56-66`, `build.rs:31-53`. The fallback is documented, but there is **no CI-time `GIT_HASH=$(git rev-parse --short HEAD)` export** visible in any workspace file (grep for `GIT_HASH=` returns only the build.rs line). A release tarball built without explicit env passing reports `pcloudc 0.0.0 (unknown, release)`.
- Remediation: document the `GIT_HASH` env override in the release runbook; add a CI job that passes the hash explicitly.
- Severity: LOW.

**CLI-L4 — `--version` does not surface Rust compiler, tls library, or feature-flag matrix.**
- Evidence: `main.rs:56-66`.
- Impact: enterprise support workflows typically need the build triple + rustc version. Current output `pcloudc 0.1.0 (abcd123, release)` is thin.
- Remediation: emit a `--version --verbose` block with `std::env::consts::OS`, `std::env::consts::ARCH`, `rustc_version_runtime`, enabled feature flags. Match the shape of `rustc -Vv`.
- Severity: LOW.

#### 8.2.4 Long-running command UX

**CLI-M8 — `mount`, `upload create`, `download file`, `verify --recursive`, `snapshot create` are long-running but `progress.rs` has no call sites outside its own tests.**
- Evidence: `progress.rs:1-454` defines `ProgressMode`, `ProgressEvent`, `StderrSink`. A `grep -rn "ProgressMode\|ProgressEvent\|ProgressSink" crates/pcloud-cli/src/` returns matches only inside `progress.rs` itself. The module is wired but not invoked.
- Impact: `pcloudc download file 12345 /tmp/huge.bin` shows nothing until it finishes. `pcloudc snapshot create /backup.tar.zst` returns when done; no progress. Enterprise UX expectation is broken.
- Remediation: wire `progress::Reporter` into `download_file`, `upload create|list` (blocking waits), `snapshot create|restore|verify`, `verify --recursive`.
- Severity: MEDIUM.

**CLI-M9 — Ctrl-C during a running IPC call leaves the socket connection in an undefined state.**
- Evidence: grep for `signal_hook\|ctrlc\|sigaction` in `crates/pcloud-cli/src/` returns only `globals::tests::trace_env_guard` and `pcloud-secret/...` references. The CLI installs no SIGINT handler; Tokio is not in use (no `#[tokio::main]`). `main.rs` is synchronous.
- Impact: for short commands this is fine. For `snapshot create` (minutes) a Ctrl-C abandons the daemon mid-work. The daemon has drain/unmount handling, but the CLI has no confirmation UX ("press Ctrl-C again to force").
- Remediation: install a SIGINT handler that clears the cursor line on spinner mode and exits with code 130 (conventional SIGINT exit code).
- Severity: MEDIUM.

**CLI-L5 — `progress.rs` NDJSON mode emits progress to *stderr* (`progress.rs:95-99`) even in `--json` mode.**
- Evidence: `progress.rs:12-16` documents this.
- Impact: in theory fine (stdout stays clean for the final envelope) but operators piping `pcloudc --json upload create ... 2>&1 | jq` get mixed line-per-event + final envelope in stderr/stdout stream interleave. Current status: no call sites (CLI-M8), so not observable.
- Severity: LOW.

#### 8.2.5 Error messages

**CLI-L6 — `CommandParseError::UnknownCommand` carries the raw user-supplied token (not sanitised).**
- Evidence: `app.rs:446-447` — `#[error("unknown command '{0}'")] UnknownCommand(String)`.
- Impact: the caller controls the string, so no secret-leak class. But a malicious operator running inside a shared CI log view could inject ANSI escape codes via `pcloudc $'\e[2Jfake-command'`. The error printing path is `report_error`, which does not filter.
- Remediation: pass tokens through a printable-ASCII sanitiser before emitting into error strings.
- Severity: LOW.

**CLI-L7 — `report_error` prints the `Internal` daemon error message verbatim, including daemon-side stack-trace tails when `tracing` is hooked in.**
- Evidence: `main.rs:438-505` uses `response.message` as the error detail. Daemon-side internal errors are opaque `"internal: <inner err.to_string()>"` strings; on observability builds this can include file:line from the daemon crate.
- Impact: internal-impl leakage in CLI output. Minor for enterprise; inconvenient for public-facing tooling.
- Remediation: when `flags.verbosity == 0`, strip internal diagnostic details and print "internal daemon error — rerun with -v for details" for `Internal` statuses.
- Severity: LOW.

---

### 8.3 SDK findings

#### 8.3.1 Public API semver discipline

**SDK-H1 — `pub use pcloud_proto::Notification` re-exports a type from a non-peer crate.**
- Evidence: `pcloud-sdk/src/lib.rs:102-105`:
  ```rust
  /// Typed notification record mirroring the C `psync_notification_t`. Re-exported
  /// from `pcloud-proto` so SDK consumers do not need a direct dependency on the
  /// protocol crate.
  pub use pcloud_proto::Notification;
  ```
  `pcloud_proto` is a workspace-internal crate (`description = "Typed pCloud protocol clients"` in its Cargo.toml). The SDK now has an `Notification: From<pcloud_proto::Notification> for pcloud_sdk::Notification` semver contract that *is actually* `pcloud_proto::Notification`. Any breaking change to the proto `Notification` struct is a SemVer break on the SDK.
- Impact: the SDK claims to be embeddable ("wraps the daemon runtime"), but a caller gets stuck on whichever `pcloud-proto` the SDK is built against. The three-crate coupling (`sdk → daemon → proto` already, plus this direct `sdk → proto`) is not clean.
- Remediation: either (a) define `pcloud_sdk::Notification` as its own struct with an explicit `From<pcloud_proto::Notification>`, or (b) document the re-export as semver-locked in the rustdoc (currently doesn't say so).
- Severity: HIGH.

**SDK-H2 — `pub use upload_session::{…}` bulk re-export includes the `UploadSessionDriver` trait, which leaks private-module design details.**
- Evidence: `pcloud-sdk/src/lib.rs:97-100`:
  ```rust
  pub use upload_session::{
      ConflictMode, DEFAULT_CHUNK_SIZE, FileMetadata, UploadConfig, UploadError, UploadHandle,
      UploadPayload, UploadProgress, UploadRequest, UploadSession, UploadSessionDriver, UploadState,
  };
  ```
  `UploadSessionDriver` is the trait the internal upload machinery uses to talk to the daemon or a mock (`tests/upload_session_chunked.rs:50`). By re-exporting it, callers can now implement their own driver — intentional? If yes, it should be documented. If no, it's an accidental leak that locks the internal shape.
- Impact: any refactor of `upload_session::UploadSessionDriver` is now a SemVer break.
- Remediation: either document the driver trait as stable plugin surface, or move the trait into `pcloud_plugin_api` and re-export explicitly, or make it `pub(crate)` and expose `MockDriver` only behind a `#[cfg(test)]` re-export.
- Severity: HIGH.

**SDK-H3 — Only ONE example (`login_and_list.rs`) for 80+ public helpers.**
- Evidence: `crates/pcloud-sdk/examples/` contains exactly one file (line count 74). Sixty-plus public methods on `EmbeddedDaemon` have `# Examples` rustdoc blocks referring to `examples/sdk_plugin_registration.rs` and `examples/sdk_upload_download.rs` — e.g. `pcloud-sdk/src/lib.rs:1300` ("See `examples/sdk_plugin_registration.rs` for a runnable demo"), line 1376 ("See `examples/sdk_upload_download.rs` for a runnable demo"). Those files **do not exist**. `cargo build -p pcloud-sdk --examples` cannot build them.
- Impact: rustdoc cross-references are broken. `cargo doc --workspace --no-deps` will not fail on missing examples (rustdoc doesn't check file-on-disk), but a developer following the docs is led to a dead end.
- Remediation: either write the two missing examples (plugin registration, upload+download), or remove the rustdoc references to non-existent files.
- Severity: HIGH.

#### 8.3.2 Rustdoc coverage

**SDK-M1 — `SdkError` is marked `#[non_exhaustive]` (semver-safe add) but **every** per-helper error enum is **also** `#[non_exhaustive]` — forcing callers to double-match fallthroughs even within the wrapped error.**
- Evidence: `pcloud-sdk/src/lib.rs:127, 258, 305, 338, 351, 362, 408, 434, 569, 583, 601, 674, 710, 752, 818`.
- Impact: callers who want to switch on both the outer and inner shape are forced to add `_ => unreachable!()` twice in every arm. The ergonomic cost is real — `SdkError` alone being non-exhaustive would deliver the same forward-compat guarantee.
- Remediation: consider removing `#[non_exhaustive]` from the inner enums (keep only on `SdkError`). Nothing *external* to the SDK constructs them.
- Severity: MEDIUM.

**SDK-M2 — `EmbeddedDaemon::dispatch` claims "infallible at the Rust level" but is the primary way callers can panic-crash the SDK when the underlying daemon panics.**
- Evidence: `pcloud-sdk/src/lib.rs:1252-1254` ("This method is infallible at the Rust level — errors are encoded in the returned `Response::status`"). But `dispatch` calls into the daemon's `dispatch(&mut self.runtime, request)` which is a synchronous call that can panic on malformed requests (no `catch_unwind`).
- Impact: an embedded daemon crash takes down the host application.
- Remediation: wrap the dispatch in `std::panic::catch_unwind` and return a synthetic `ResponseStatus::InternalError` on panic. Document this clearly.
- Severity: MEDIUM.

**SDK-M3 — `crypto_priv_key_flags` returns `u64` with no `Result`; how does it signal absence?**
- Evidence: `pcloud-sdk/src/lib.rs:1860`: `pub fn crypto_priv_key_flags(&self) -> u64`. No error path, no `Option`. When the crypto shell has never been set up, what does this return? `0`?
- Impact: callers cannot distinguish "flags == 0" (valid legitimate state) from "crypto not initialised" (caller error).
- Remediation: return `Result<u64, SdkError>` (wrap in a new `CryptoHelperError::NotSetup` variant) or `Option<u64>`.
- Severity: MEDIUM.

**SDK-L1 — Benches exist but are under-covered.**
- Evidence: `crates/pcloud-sdk/benches/upload_session.rs` (3.8 KB); no `crates/pcloud-sdk/benches/dispatch.rs` for the embedded-daemon dispatch hot path.
- Impact: performance regressions on `EmbeddedDaemon::dispatch` will not be caught by CI.
- Remediation: add a trivial `health_ping_throughput` bench that tracks dispatch latency.
- Severity: LOW.

**SDK-L2 — `CRATE_NAME` public constant (`lib.rs:94`) is duplicated at every caller that does telemetry tagging instead of being a single-source-of-truth pattern.**
- Evidence: `lib.rs:94`: `pub const CRATE_NAME: &str = "pcloud-sdk";`. The doctest: `assert_eq!(pcloud_sdk::CRATE_NAME, "pcloud-sdk");`.
- Remediation: use `env!("CARGO_PKG_NAME")` at definition site.
- Severity: LOW.

#### 8.3.3 Tests

**SDK-M4 — SDK tests cover only the happy path for the upload session state machine.**
- Evidence: only test file is `tests/upload_session_chunked.rs`. There are no tests for: `verify_email`, `register`, `lost_password`, `change_password`, `create_backup`, `delete_backup`, `send_publink`, `set_api_server`, `userinfo`, `logout`, `send_two_factor_sms`, `download_file`, `stat_path`, `list_folder`, `mount`/`unmount`, or any of the 48 typed KV / setting helpers.
- Impact: the 80+ public helper methods are exercised only through integration tests elsewhere in the workspace. An SDK-specific test suite that embeds `EmbeddedDaemon` against a mock transport does not exist.
- Remediation: add `tests/sdk_smoke.rs` that bootstraps an `EmbeddedDaemon` with a mock `Environment::Development` profile and exercises every `Result`-returning helper for its `NotAuthenticated` branch (trivial; no network). This catches signature drift.
- Severity: MEDIUM.

**SDK-L3 — `login_and_list.rs` uses `env::remove_dir_all` with `let _ =` for cleanup — silently swallows I/O errors.**
- Evidence: `login_and_list.rs:71`: `let _ = std::fs::remove_dir_all(&root);`.
- Impact: if the cleanup fails (ENFILE, permission denied), the scratch directory lingers under `$TMPDIR` and the example exits Ok. Accumulating scratch directories on long-running CI hosts.
- Remediation: propagate with `?` or log the failure.
- Severity: LOW.

#### 8.3.4 Feature flags

**SDK-M5 — `pcloud-sdk/Cargo.toml` declares NO features at all.**
- Evidence: full `[features]` section is absent. `tokio` is pulled in with `default-features = false, features = ["sync"]` hard-coded (line 20).
- Impact: no `tls-rustls` vs `tls-native-tls` alternatives; no way for a downstream embedder to opt out of `rustls` and use the system TLS. The TLS provider is hard-coded upstream through `pcloud-proto` (which uses `rustls = "0.23"` with `features = ["std", "ring"]` at `pcloud-proto/Cargo.toml:24`).
- Impact details:
  - The whole workspace is **ring-only**. No `rustcrypto` provider path. A FIPS-conscious enterprise that wants `aws-lc-rs` or `rustls-platform-verifier` has no knob.
  - `webpki-roots = "1.0"` (`pcloud-proto/Cargo.toml:30`) hard-codes the root bundle. No way to use the system trust store.
- Remediation: expose feature flags `tls-rustls-ring` (default), `tls-rustls-aws-lc`, `tls-native` by plumbing through to `pcloud-proto` and `pcloud-daemon`. Also add `plugin-api` as an optional feature (currently unconditional dep, line 15).
- Severity: MEDIUM.

**SDK-L4 — `pcloud-plugin-api` is a *required* dep of `pcloud-sdk` — every SDK consumer pulls in ed25519-dalek.**
- Evidence: `pcloud-sdk/Cargo.toml:15`: `pcloud-plugin-api = { path = "../pcloud-plugin-api" }` (unconditional). `pcloud-plugin-api/Cargo.toml:15`: `ed25519-dalek = "2.1"`.
- Impact: a pure-upload embedder (who never touches plugins) still compiles ed25519-dalek + its crypto transitively. Cold build-time cost on small embedders.
- Remediation: gate `pcloud-plugin-api` behind a `plugin` feature and conditionally-compile `register_plugin`, `authorize_plugin_operation`, `loaded_plugins` on that feature.
- Severity: LOW.

#### 8.3.5 Re-exports / prelude

**SDK-M6 — No prelude module; `use pcloud_sdk::*` imports 30+ types/enums at once.**
- Evidence: `pcloud-sdk/src/lib.rs` has no `pub mod prelude` or curated narrow re-export. A glob import inherits every one of `ConflictMode`, `UploadConfig`, `UploadPayload`, `FileMetadata`, `UploadProgress`, `UploadHandle`, `UploadState`, `UploadError`, `DEFAULT_CHUNK_SIZE`, `Notification`, plus `EmbeddedDaemon`, `EmbeddedDaemonBuilder`, 14 helper error enums, `SdkError`, `AuthenticatedUser`, `DownloadLinkInfo`, `TwoFactorSmsInfo`, `TwoFactorNotificationInfo`, `UploadResult`, `PromoResult`, `ApiServerResult`, `BackupCreated`, `FilesystemPathStatus`, `FolderFlagsInfo`, `StatResult`, `FolderEntry`, `CreateFolderResult`, `CRATE_NAME`.
- Impact: name collisions in host crates (especially `Notification`, `FolderEntry`, `StatResult`) cause compile errors in embedders that already have domain types.
- Remediation: expose `pub mod prelude { pub use crate::{EmbeddedDaemon, SdkError, UploadRequest, UploadResult}; }` and document it as the recommended glob. Existing types remain reachable through `pcloud_sdk::Type`.
- Severity: MEDIUM.

**SDK-L5 — `EmbeddedDaemon::config(&self) -> &ConfigProfile` exposes the internal config type from a *peer* crate.**
- Evidence: `pcloud-sdk/src/lib.rs:1237`. `ConfigProfile` comes from `pcloud-config`, which is a workspace-internal crate.
- Impact: same SemVer-lock risk as SDK-H1, but `ConfigProfile` is a fatter struct with more fields.
- Remediation: either lift `ConfigProfile` into `pcloud-sdk` as a thin facade (`impl From<ConfigProfile> for SdkConfig`), or document the peer-crate contract.
- Severity: LOW.

---

### 8.4 Cross-cutting findings

**X-H1 — `pcloud-cli/Cargo.toml` declares the `derive` feature of clap but then hand-rolls the parser.**
- Evidence: Cargo.toml line 27 includes `"derive"` in `clap` features. No `#[derive(Parser)]` block exists.
- Impact: one unused compile unit for a feature flag that would be better used.
- Remediation: either migrate to clap-derive (preferred; fixes CLI-H2, CLI-H3, CLI-H5 in one stroke), or drop the feature from Cargo.toml.
- Severity: HIGH.

**X-H2 — The CLI help text at `app.rs:16-442` (~425 lines) is a hand-maintained triple-truth for subcommands (it joins `Command` variants + `app::parse_command` tables + clap completion). Drift between the three is the norm.**
- Evidence:
  - The help text advertises `stop` (line 86), `f` (line 90), `crypto status` (line 273), `slo` (via examples line 419). Neither `stop`, `crypto status`, nor `slo` appear in `completion::build_cli`.
  - The help text does NOT document `notifications` fully, `snapshot`, `integrity`, `ha`, `audit-verifier`, `upload`, `conflict`, `stat`.
- Impact: help text is partially stale and diverges from real capability.
- Remediation: single source of truth (clap-derive) OR move help text into per-variant `#[doc]` attributes on `Command` and have `help_text()` auto-synthesise.
- Severity: HIGH.

**X-M1 — `pcloud-sdk` and `pcloud-cli` do not share a `pcloud-error::Category` contract for exit-code derivation.**
- Evidence: SDK funnels every error through `pcloud_error::Category` (`pcloud-sdk/src/lib.rs:930-1120`); CLI maps `ResponseStatus` → `ExitCode` directly (`exit_code.rs:97-137`). These are two separate category ladders. An SDK consumer writing a CLI-clone cannot reuse the SDK's category and get identical exit codes out of the box.
- Impact: cohesion across the two interfaces is weak. Any host embedding the SDK has to re-implement exit-code mapping.
- Remediation: provide `pcloud_sdk::exit_code_for(category: Category) -> u8` as part of the SDK's public surface.
- Severity: MEDIUM.

**X-M2 — The CLI prints a traceparent to stderr before every command (`main.rs:417-421`) but the SDK `EmbeddedDaemon::dispatch` has no trace-context API.**
- Evidence: `main.rs:429-436` uses `RequestEnvelope::new(request).with_traceparent(tp)` but `EmbeddedDaemon::dispatch` takes a bare `Request` (`lib.rs:1271`).
- Impact: trace-context propagation is CLI-only. Embedded SDK consumers cannot propagate their trace id through to the daemon audit log.
- Remediation: add `EmbeddedDaemon::dispatch_envelope(&mut self, envelope: RequestEnvelope)` taking the full envelope.
- Severity: MEDIUM.

**X-M3 — No `#[deny(warnings)]` / `#[forbid(unsafe_code)]` on `pcloud-cli/src/main.rs`; `unsafe_code` is used for `pre_exec`, `setsid`, and signal handlers.**
- Evidence: `main.rs:2-4`. Multiple `#[allow(unsafe_code)]` attributes on modules (lines 18, 20, 24, 28, 35). `#[warn(unsafe_op_in_unsafe_fn)]` is set (line 2), which is good.
- Impact: unsafe usage is explicit but scattered; no single place enumerates the five unsafe call sites.
- Remediation: either consolidate unsafe into a single `unix_daemon` module with a safety invariant block, or add an allow-list audit test that grep-asserts the count of `unsafe { … }` blocks (regression tripwire).
- Severity: MEDIUM.

**X-L1 — Plugin API (`pcloud-plugin-api`, `pcloud-plugin-autoheal`, `pcloud-plugin-backup-schedule`, `pcloud-plugin-dlp`, `pcloud-plugin-publink-expiry`) has five crates but the SDK re-exports `Plugin` trait only.**
- Evidence: `pcloud-sdk/src/lib.rs:82-84` imports `Plugin, PluginAuditEvent, PluginAuditSink, PluginError, PluginOperation, PluginRegistry, RegisteredPlugin`. Only `Plugin` would normally be what a plugin-author needs; the SDK offers no prelude for plugin authoring. A plugin author needs to depend directly on `pcloud-plugin-api`.
- Impact: the plugin authoring story is unclear from the SDK alone.
- Remediation: add `pcloud_sdk::plugin` module that re-exports the plugin-author-facing subset.
- Severity: LOW.

**X-L2 — No `pcloud-sdk/examples/README.md` and no list of examples in the SDK README.**
- Evidence: `crates/pcloud-sdk/README.md` exists but wasn't inspected in this audit for example coverage. The examples/ directory contains only one file.
- Remediation: document which examples exist and which are planned.
- Severity: LOW.

---

### 8.5 Prioritised remediation roadmap

**Must-fix before GA:**
1. CLI-H2 / CLI-H3 / CLI-H5 / X-H1 / X-H2 — **consolidate on clap-derive** or write a test that asserts parse ↔ help ↔ completion alignment. Biggest UX win. Eliminates four HIGH findings in one refactor.
2. CLI-H1 — wire `Method::Health` (enterprise probe) into the CLI. Single-site fix.
3. CLI-H4 — add `crypto setup` + `crypto mkdir` subcommands.
4. CLI-H6 + CLI-M7 — deprecate all positional-secret subcommands; print a consistent stderr warning today, reject in the next major.
5. SDK-H3 — either write the two missing examples or strip the rustdoc references to them.
6. SDK-H1 / SDK-H2 — audit every `pub use` and either wrap with a thin SDK-owned facade or document the semver contract with the upstream crate explicitly.

**Should-fix:**
7. CLI-M1 / CLI-M4 / CLI-M5 / CLI-M6 / CLI-M8 / CLI-M9 — exit-code discipline, missing per-root pause/resume, value-KV CLI, progress wiring, SIGINT.
8. SDK-M1 / SDK-M2 / SDK-M3 / SDK-M4 / SDK-M5 / SDK-M6 — `#[non_exhaustive]` ergonomics, panic-safety, error path, tests, feature flags, prelude.
9. X-M1 / X-M2 / X-M3 — exit-code cohesion, trace-context propagation, unsafe audit.

**Nice-to-have:**
10. All LOW findings (CLI-L*, SDK-L*, X-L*) can be batched into a "CLI polish" milestone.

---

### 8.6 Evidence summary (file:line anchors referenced in this section)

- CLI workspace: `crates/pcloud-cli/Cargo.toml:27` (clap-derive declared), `crates/pcloud-cli/build.rs:20-53` (GIT_HASH injection).
- CLI parser: `crates/pcloud-cli/src/main.rs:46` (entry), `crates/pcloud-cli/src/app.rs:1474` (`parse_command`), `crates/pcloud-cli/src/app.rs:1484` (`parse_inputs_for_command`), `crates/pcloud-cli/src/commands.rs:35` (`enum Command`), `crates/pcloud-cli/src/commands.rs:750` (`into_request`).
- CLI help: `crates/pcloud-cli/src/app.rs:16-442` (hand-maintained man page).
- CLI completion: `crates/pcloud-cli/src/completion.rs:35-193` (parallel clap tree), 215-284 (tests that only verify non-emptiness).
- CLI exit codes: `crates/pcloud-cli/src/exit_code.rs:58-87` (stable-ABI enum).
- CLI secrets: `crates/pcloud-cli/src/prompt.rs:59-144` (SecretPrompt), `crates/pcloud-cli/src/app.rs:2924-3020` (`read_password_securely`), `crates/pcloud-cli/src/app.rs:1527-1533` (missing warning on submit-auth positional).
- CLI progress: `crates/pcloud-cli/src/progress.rs:1-454` (declared, unused in production paths).
- IPC truth: `crates/pcloud-ipc/src/methods.rs:37-216` (`enum Method`, 42 variants), 262-1021 (`enum Request`, 70+ variants).
- SDK surface: `crates/pcloud-sdk/src/lib.rs:94` (`CRATE_NAME`), 97-105 (re-exports), 111-138 (`EmbeddedDaemon` / `Builder` / `Error`), 819-918 (`SdkError`), 1122-3400+ (helper methods).
- SDK examples: `crates/pcloud-sdk/examples/login_and_list.rs` (only existing example, 74 lines).
- SDK tests: `crates/pcloud-sdk/tests/upload_session_chunked.rs` (only SDK-specific test).
- SDK features: `crates/pcloud-sdk/Cargo.toml:1-35` (no `[features]` section).
- TLS provider chain: `crates/pcloud-proto/Cargo.toml:24` (rustls-ring hard-coded), `crates/pcloud-proto/Cargo.toml:30` (webpki-roots hard-coded).
- Plugin API: `crates/pcloud-plugin-api/Cargo.toml:1-19` (unconditional ed25519-dalek dep).

---

*End of Section 8.*

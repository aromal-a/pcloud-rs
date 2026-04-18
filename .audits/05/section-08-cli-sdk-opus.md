# Section 8 — CLI & SDK Audit (Opus, Audit 05)

Scope: `pcloud-cli/src/{app.rs, commands.rs, completion.rs, crypto_setup_picker.rs}`,
`pcloud-sdk/src/lib.rs`. Focus on argv secret hygiene, Wave 2 crypto-backend
UX (`crypto setup --backend {pclsync-compat|enhanced}`, `--acknowledge-not-interop`,
`get-folder-key`, `get-file-key`, literal `YES` picker), and SDK surface / semver.

## Summary

The Wave 2 UX (dual-backend picker, argv password gate, key-debug helpers) is
correctly wired and the secret wrappers are used consistently. Remaining risks
are MEDIUM/LOW: the `--allow-argv-password` opt-in is honest but cannot scrub
`/proc/self/cmdline`; `get-folder-key` / `get-file-key` exist on the IPC
surface without a documented redacted-print contract; shell completion for
Wave 2 flags is incomplete; the SDK has no semver posture or rustdoc
`# Examples` gate.

## Findings

### CRITICAL
None.

### HIGH
None.

### MEDIUM

- **M1 `crypto get-folder-key` / `get-file-key` — missing output-safety
  contract.** `app.rs:662-663, 2841-2868` and `commands.rs:578-588, 1272-1277`
  wire these straight to `Request::CryptoGetFolderKey` /
  `Request::CryptoGetFileKey`. The help text (`app.rs:281-284`) calls them
  "debugging helpers" that "fetch + cache a folder's wrapped sym-key", but
  there is no unit test or source comment in either CLI dispatch or the
  generic response printer that asserts the daemon reply is *not* rendered
  as hex/base64 key material on stdout. A debug helper that can exfiltrate
  wrapped key material via shell redirection (tee/log/CI) needs either
  (a) an explicit `--show-cache-only` flag that only prints a cache
  hit/miss boolean, or (b) a redacted printer with a test that fails if
  raw key bytes leak. Recommend: add a `printer_redacts_wrapped_key` test
  in `commands.rs` and gate raw output behind `PCLOUD_DEBUG_KEY_MATERIAL=1`
  with a stderr warning.

- **M2 `--allow-argv-password` cannot scrub `/proc/self/cmdline`.**
  `app.rs:3338-3366` correctly hard-fails absent the opt-in and wraps the
  value in `SecretString`, but the comment at `app.rs:3356-3364` honestly
  admits the caller-owned argv is not mutated here ("we only have a
  `&[String]` here we can't mutate"). The kernel-maintained `cmdline`
  copy survives until process exit. The warning text is accurate; however
  the code does not call `prctl(PR_SET_MM_ARG_START/END)` on Linux, and
  does not `zeroize` the caller's `String` argv in `main.rs`. Either
  document this as "accepted residual exposure" in `OPERATIONS-RUNBOOK.md`
  or land a Linux-gated argv scrub path. Current state is a MEDIUM
  residual because `--allow-argv-password` is opt-in and user-acknowledged.

- **M3 Shell completion (`completion.rs`) Wave 2 coverage is partial.**
  `completion.rs:86-134` covers `crypto setup` with `--backend` and
  `--acknowledge-not-interop`, plus `get-folder-key`/`get-file-key` as
  bare subcommands — but the `get-folder-key` / `get-file-key` nodes
  (lines 123-134) have no positional `<FOLDER_ID>` / `<FILE_ID>` argument
  declared, so bash/zsh/fish complete the flag name but not the expected
  numeric operand. Also missing: completion entries for
  `--password-stdin`, `--password-env`, `--allow-argv-password` global
  flags that `app.rs:155-158, 3343` parse. Add `.arg(Arg::new("id")
  .required(true))` to both crypto subcommands and surface the three
  password-handling flags at the root.

### LOW

- **L1 SDK has no declared semver posture.** `pcloud-sdk/Cargo.toml`
  uses `version.workspace = true` (workspace root pins `0.x`). `lib.rs`
  has `//! # Examples` (line 52) and an `examples/` dir with five
  runnable examples — good — but there is no `#[deprecated]` migration
  lane, no `CHANGELOG` link from the crate docs, and no
  `#![deny(missing_docs)]`. For an "embeddable SDK surface" this is
  acceptable at 0.x but should gain `missing_docs` + a documented
  `MSRV` + "0.x means breaking changes allowed between minors"
  statement in the crate-level doc before any 1.0 claim.

- **L2 Picker race / TOCTOU on tty detection.** The picker is only
  supposed to run on a tty (`crypto_setup_picker.rs:9-12` comment), but
  the actual tty check lives in `app.rs` (not shown here) and is not
  re-asserted inside `run_picker`. If a future refactor calls
  `run_picker` from a non-tty path, the menu writes to stdout and the
  `YES` check still works — no security break, but the Stage 4b.4 spec
  ("non-interactive scripted path rejects before picker") would be
  silently violated. Recommend: take an `is_tty: bool` parameter and
  `debug_assert!(is_tty)` in `run_picker`.

- **L3 `YES` confirmation is single-attempt.** `crypto_setup_picker.rs:118-124`
  aborts on any non-`YES` answer including typos. Consistent with spec
  ("strict: only literal `YES`"), but the menu loop gives 3 retries
  (`MAX_RETRIES = 3`, line 51) while the confirmation gives zero. A
  fat-fingered lowercase `yes` forces a full `crypto setup` restart.
  Consider one retry with "Type YES (case-sensitive) or anything else
  to abort:". Not a security issue — strictness is intentional.

- **L4 `commands.rs:807` has a stray single-slash doc comment** (`/ Optional
  passphrase hint…`) — rustdoc drops the line silently. Cosmetic but
  reduces generated docs.

## Positive observations

- `SecretString` / `SecretPrompt` used uniformly for login, submit-password,
  change-password, and crypto setup (`app.rs:1584, 2639, 3335-3368`).
- Picker correctness is well-tested: `choice_2_requires_yes_in_full_caps`
  and `choice_2_aborts_on_lowercase_yes` exist (`crypto_setup_picker.rs`
  test module, lines 176, 198).
- Argv-password gate is fail-closed (`app.rs:3343-3350`, exit 2) with
  accurate stderr disclosure of the `/proc/<pid>/cmdline` residual.
- SDK re-exports typed protocol records (`lib.rs:~102-127`) so consumers
  avoid direct `pcloud-proto` coupling.
- Password-priority chain documented and implemented in priority order:
  stdin → env → interactive → argv (`app.rs:1579-1584`).

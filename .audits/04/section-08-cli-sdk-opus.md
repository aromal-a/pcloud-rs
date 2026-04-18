# Section 8 Audit — CLI & SDK (Auditor: Opus)

Scope: `pcloud-cli/src/{app,commands,completion}.rs` and `pcloud-sdk/src/lib.rs`.
Specific focus: newly-landed `pcloudc upload from-file` and `publink create-tree-from-paths` parity wiring (bd-1du.10 rows 93 & 149).

## Verification of new surfaces (bd-1du.10)

- Row 93 (`upload_writefromfile`): argv `upload from-file` dispatches to `Command::UploadFromFile`
  (`app.rs:841`, `app.rs:2757-2783`), reaches `Request::UploadWriteFromFile`
  (`commands.rs:1207-1212`). IPC variant + daemon dispatch + proto
  encoder + transfer backend all present. **Wired end-to-end.**
- Row 149 (`ptree_public_link` path-based): argv `create-tree-link-from-paths`
  dispatches to `Command::CreateTreeLinkFromPaths` (`app.rs:1492`,
  `app.rs:2744-2756`) and emits `Request::CreateTreePublicLinkFromPaths`
  (`commands.rs:1313-...`). Daemon-side path resolution under auth
  context. **Wired end-to-end.**

Both Audit-03 Partial rows are closeable on CLI/IPC surface.

## Findings

### HIGH

- **H1. New commands missing from shell completion.**
  `pcloud-cli/src/completion.rs:209-221` declares `upload` subcommands
  `create|pause|resume|cancel|list` but no `from-file`. The top-level
  subcommand list enumerates `create-tree-link` (line 150) but not
  `create-tree-link-from-paths`. bash/zsh/fish users cannot tab-complete
  the two new verbs, and a Section-8 parity-closure claim that advertises
  "all subcommands reach daemon via CLI" is inconsistent with an
  incomplete completion surface. Add both entries.

- **H2. SDK lacks typed helpers for the two new operations.**
  `pcloud-sdk/src/lib.rs` has 52 `pub fn` surfaces but neither
  `upload_write_from_file` nor `create_tree_public_link_from_paths`
  appears (grep: 0 matches at any line). Direct `dispatch(Request::…)`
  works, but embedders have no typed, documented, result-mapped entry
  point. `#![deny(missing_docs)]` at `lib.rs:67` does not catch this
  because the methods simply don't exist. Add SDK wrappers so
  `bd-1du.10` "SDK breadth" matches the CLI breadth.

### MEDIUM

- **M1. `upload from-file` positional prompts misuse `SecretPrompt`.**
  `app.rs:2763, 2770` build interactive prompts via `SecretPrompt::new`
  for non-secret values (session id, local path). `SecretPrompt` is the
  redacted/zeroised wrapper used for passwords; using it for
  session-id/path gives users the no-echo UX of a password prompt and
  confuses operators. Use a plain prompt helper (see analogous
  non-secret prompts in `parse_inputs_for_command` for numeric IDs
  elsewhere) and keep `SecretPrompt` reserved for credential paths.

- **M2. `create-tree-link-from-paths` has no argv-length guard.**
  `app.rs:2751` happily accepts an empty `paths: Vec<String>` (tail
  slice default) and forwards a zero-path request to the daemon. There
  is no client-side reject for "no paths supplied", forcing the daemon
  to synthesize an error. Reject `paths.is_empty()` with
  `invalid_input("create-tree-link-from-paths: at least one path required")`.

- **M3. `UploadFromFile` local-path is not validated before IPC.**
  `app.rs:2768-2771` reads a raw `String` and ships it to the daemon
  without any existence/readability precheck, while analogous
  `upload create` in `commands.rs` pairs `--file <PATH>` with a
  resolver. For a better failure mode and to keep daemon-side errors
  cheap, canonicalise/stat the path client-side and reject non-files
  before dispatch, matching the defensive posture the rest of the CLI
  uses (`parse_inputs` test cluster at `app.rs:3239+`).

### LOW

- **L1. Help text not updated.** The hand-written help block
  (`app.rs:~105-432`) describes `upload create/pause/resume/cancel` but
  I did not find a `from-file` / `create-tree-link-from-paths` entry.
  Users discover the commands only via source-reading or error paths.
  Add a one-line description to each section.

- **L2. Semver discipline — no `#[non_exhaustive]` on SDK public
  enums/structs.** `pcloud-sdk/src/lib.rs` adds new surfaces
  aggressively (10+ methods in recent waves). Consider
  `#[non_exhaustive]` on public response structs that evolve with
  server shape changes (`StatResult`, `FolderEntry`, `PromoResult`,
  `AuthenticatedUser`) to keep future additive fields from being a
  semver break.

- **L3. Example coverage gap.** `crates/pcloud-sdk/examples/` has
  `crypto_lifecycle`, `login_and_list`, `public_link`,
  `upload_and_download` but no example demonstrating resumable
  chunked upload via `upload_write_from_file` (once H2 is addressed)
  or tree-link-from-paths. Given `deny(missing_docs)` is on, having
  worked examples matters for doc quality claims.

## Argv secret exposure — clean

`--allow-argv-password` gating is enforced at `app.rs:1553, 1576, 1599,
3158` for `submit-password`, `auth`, and login flows. `normalize_args`
+ `flag_takes_value` (`app.rs:458-488`) properly partition
`--password-env`, `--password-stdin`, and the TOTP/recovery variants.
No regressions found. `SecretString` wrappers flow from prompt through
to `SecretInputs`. Zeroisation path intact.

## Summary

Both new parity-closing commands are correctly wired in the CLI→IPC→
daemon path. Closing bd-1du.10 additionally requires: completion
(H1), SDK typed helpers (H2), and the path/empty guards (M1-M3).
Argv-secret exposure posture remains strict.

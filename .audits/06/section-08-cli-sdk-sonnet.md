# Section 8: CLI & SDK — Sonnet Audit 06 (Cross-validator)

**Date:** 2026-04-18  
**Auditor:** Sonnet 4.6 — independent cross-validator  
**Scope:** `crates/pcloud-cli/`, `crates/pcloud-sdk/` — verifying audit-05 Sonnet findings (completion drift, `get-folder-key`/`get-file-key` positionals, `env::remove_var` SAFETY, argv-password help text, fork URL)

---

## Status of Audit-05 Findings

### M-8.1 — Completion tree omits sync subcommands (audit-05 MEDIUM) — **FIXED**

`crates/pcloud-cli/src/completion.rs:84-124` now registers `sync change-type`, `sync localscan`, `sync suggest`, `sync is-syncable`, and `sync status` with their positional args. Each block carries a `// M-8.1:` marker confirming intentional addition. The audit-05 gap is closed.

**Residual (new, MEDIUM):** Despite the sync block fix, the following `Command` enum variants from `commands.rs` remain absent from the `build_cli()` completion tree:

- `DownloadFile` / `DownloadLink` — no `download` subcommand in completion (these appear only as `download-only` in a `value_parser`; `completion.rs:95`)
- `FileHistory` / `FileDiff` / `FileRestore` — no `log`, `file-history`, `file-diff`, or `file-restore` entries anywhere in `completion.rs`
- `AccountVerifyEmail` / `AccountVerifyEmailRestricted` / `AccountLostPassword` / `AccountChangePassword` / `AccountRegister` / `AccountApiServers` / `AccountSetApiServer` / `AccountSetLanguage` / `AccountPromo` — no `account` subcommand group in the completion tree (only `account-stopshare`, `account-modifyshare`, `account-teamshare` as flat top-level entries)
- Crypto sub-subcommands `reset`, `priv-key-flags`, `send-change-private`, `change-password`, `change-password-unlocked`, `hint` — none registered under `crypto` in the completion tree; only `start`, `stop`, `status`, `setup`, `get-folder-key`, `get-file-key` are present

The deterministic test in `completion.rs:498-523` does not walk `Command` variants exhaustively; it spot-checks 18 top-level names and would not catch any of the above gaps. Completion drift continues for roughly 15-20 command variants.

**File:line:** `crates/pcloud-cli/src/completion.rs:79-465` (entire `build_cli` body)  
**Severity:** MEDIUM

---

### M-8.2 — `get-folder-key` / `get-file-key` positionals absent from completion (audit-05 LOW) — **FIXED**

`completion.rs:165-244` now adds `folder-id` (required `u64`) and `file-id` (required `u64`) positional args plus `--root`, `--password-stdin`, `--password-env`, `--allow-argv-password` to both `get-folder-key` and `get-file-key` subcommands. Blocks are marked `// M-8.2:`. Finding closed.

---

### M-8.3 — `unsafe env::remove_var` SAFETY comment (audit-05 MEDIUM) — **SUBSTANTIVELY IMPROVED, residual LOW**

`crates/pcloud-cli/src/main.rs:2075-2093` now carries an extensive `SAFETY (M-8.3)` comment (nine lines) documenting the three invariants: pre-Tokio-runtime, no rayon/`spawn` threads, single-threaded prompt path. The comment also includes a forward-looking warning to relocate the call if async entry is adopted. The `// SAFETY:` annotation is present and explicit.

**Residual:** The invariant remains unverified mechanically (no `static_assertions` guard, no `#[cfg(test)]` thread-count assertion). A future commit that initialises a Tokio runtime before credential resolution would silently violate the comment without a compile-time error. This is acceptable at the current maturity level but should be noted.

**Severity:** LOW (reduced from MEDIUM — comment is substantially improved)

---

### M-8.4 — argv-password help text missing `/proc/self/cmdline` warning (audit-05 implicit) — **FIXED**

`crates/pcloud-cli/src/app.rs:162-170` now includes:

```
// M-8.4: surface /proc/self/cmdline leak warning in help text for --allow-argv-password.
"      --allow-argv-password Acknowledge the security risk of passing a\n",
"                           password as a command-line argument. The\n",
"                           password is visible to all processes on the\n",
"                           host via /proc/self/cmdline (Linux) and\n",
"                           shell history. Accepted ONLY for backward-\n",
```

The same security warning appears in `main.rs:1585-1635` at the parsing site, and in `completion.rs:199-204` for the `get-folder-key`/`get-file-key` `--allow-argv-password` completion entries. Finding closed.

---

### Fork URL — `Cargo.toml` `repository`/`homepage` (audit-05 MEDIUM) — **FIXED**

`Cargo.toml:63-64` now reads:

```toml
repository = "https://github.com/ezechiel203/pcloud-rs"
homepage   = "https://github.com/ezechiel203/pcloud-rs"
```

Workspace-root metadata is correct. The per-crate `Cargo.toml` files for `pcloud-cli` and `pcloud-sdk` inherit from the workspace and do not override, so they pick up the corrected URL.

**Residual (MEDIUM):** The `docs/book/src/` pages still contain numerous `pcloudcom/pcloud-rs` references — at minimum `introduction.md:6,168`, `faq.md:6,127,246`, `getting-started/install.md:94,109,246,269,309,365`, `archive/index.md:11,25`, `adr/index.md:4`, `getting-started/first-sync.md:602`. These are user-facing install instructions that point readers to clone/download from the upstream C tree. The `Cargo.toml` fix is correct; the book content is not yet corrected.

**File:line:** `docs/book/src/getting-started/install.md:109,365` (most harmful — `git clone` URL), `docs/book/src/introduction.md:6`, `docs/book/src/faq.md:6`  
**Severity:** MEDIUM (docs/book URLs unaddressed)

---

## New Findings (independent cross-validation)

### MEDIUM — `pcloudc --version` omits git SHA when `build.rs` is absent from CI

`crates/pcloud-cli/src/main.rs:62-70` uses `option_env!("GIT_HASH")` injected by a `build.rs` file. The `build.rs` exists (`crates/pcloud-cli/build.rs`). However, if the build environment does not run `git describe` (e.g., source tarball, Nix sandbox, Dockerfile without `.git`), `GIT_HASH` is absent and the banner silently falls back to `"unknown"`. This is documented behaviour (`unwrap_or("unknown")`), but operators have no indication that the SHA is missing, making crash reproducibility harder. The fallback is graceful but untestable without CI evidence.

**File:line:** `crates/pcloud-cli/src/main.rs:62-70`, `crates/pcloud-cli/build.rs`  
**Severity:** LOW

### MEDIUM — `FileHistory` / `FileDiff` / `FileRestore` stubs in completion gap create false discoverability

`commands.rs:367-375` defines `FileHistory`, `FileDiff`, `FileRestore` as real `Command` variants. `app.rs` wires them to `log`, `diff`, `restore` subcommands. None appear in `completion.rs`. `FileHistory` reaches the daemon (`Request::FileHistory`) but the daemon returns `Unavailable` with a tracker pointer. `FileDiff`/`FileRestore` are CLI-only stubs that always exit `Unavailable`. Users who discover these via `--help` but not via tab-completion get an inconsistent discovery experience; stub commands that always return `Unavailable` should either be suppressed from the help/completion surface or promoted to `Rejected` in the matrix.

**File:line:** `crates/pcloud-cli/src/commands.rs:367-375`, `crates/pcloud-cli/src/completion.rs` (absent)  
**Severity:** MEDIUM

### LOW — Completion `build_cli_has_expected_top_level_subcommands` test is structurally insufficient

`completion.rs:498-523` asserts 18 top-level names but does not assert that every `Command` variant has a corresponding completion entry. Any future addition to `commands.rs` will silently miss completion. The test provides false assurance: it passes even though `download`, `account`, `log`, and crypto sub-subcommands are absent.

**File:line:** `crates/pcloud-cli/src/completion.rs:498-523`  
**Severity:** LOW

### LOW — `get-folder-key` / `get-file-key` completion description does not signal key sensitivity

Audit-05 LOW finding still open. `completion.rs:167-168,208-209` still reads `"Fetch + cache a folder's wrapped sym-key (debugging helper)"`. The description should communicate that this returns wrapped key material and requires an unlocked crypto session.

**File:line:** `crates/pcloud-cli/src/completion.rs:167-168,208-209`  
**Severity:** LOW

---

## Summary Table

| Severity | Finding | Status | File:line |
|----------|---------|--------|-----------|
| MEDIUM | Completion tree still omits ~15-20 Command variants (`download`, `account`, `file-history`, crypto sub-subcommands) | **NEW / OPEN** | `completion.rs:79-465` |
| MEDIUM | `docs/book/src/` install/intro/faq URLs still point to `pcloudcom/pcloud-rs` despite `Cargo.toml` fix | **RESIDUAL / OPEN** | `docs/book/src/getting-started/install.md:109,365` |
| MEDIUM | `FileHistory`/`FileDiff`/`FileRestore` stub commands in completion gap with always-`Unavailable` runtime | **NEW / OPEN** | `commands.rs:367-375` |
| MEDIUM | Sync completion block fixed (M-8.1) | **FIXED** | `completion.rs:84-124` |
| MEDIUM | `Cargo.toml` repository/homepage fixed | **FIXED** | `Cargo.toml:63-64` |
| MEDIUM | argv-password `/proc/self/cmdline` help text added (M-8.4) | **FIXED** | `app.rs:162-170` |
| MEDIUM | `unsafe remove_var` SAFETY comment improved (M-8.3) | **FIXED / LOW residual** | `main.rs:2075-2093` |
| LOW | `get-folder-key`/`get-file-key` positionals added (M-8.2) | **FIXED** | `completion.rs:165-244` |
| LOW | `get-folder-key`/`get-file-key` completion description omits key-sensitivity note | **OPEN** | `completion.rs:167-168` |
| LOW | Completion test is structurally insufficient (spot-check only) | **NEW / OPEN** | `completion.rs:498-523` |
| LOW | `--version` SHA silently falls back to `"unknown"` in tarball builds | **NEW / OPEN** | `main.rs:62-70` |

**CRITICAL:** 0  
**HIGH:** 0  
**MEDIUM:** 3 open (2 residual, 1 new)  
**LOW:** 3 open

The principal quality gap post-audit-05 is the completion tree, which was partially fixed (sync block, `get-folder-key`/`get-file-key` positionals) but remains structurally incomplete for the `download`, `account`, `file-history`/`diff`/`restore`, and crypto (`reset`, `priv-key-flags`, `send-change-private`, `change-password`, `hint`) subcommand families. The fork URL fix landed in `Cargo.toml` but was not propagated to the mdBook source. No security regressions were found; the `--password-env` scrub, `SecretString` discipline, and SDK `#![deny(missing_docs)]` enforcement remain correct.

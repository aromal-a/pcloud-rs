# Section 8: CLI & SDK Audit — Sonnet (Audit 05)

Date: 2026-04-19  
Auditor: Sonnet 4.6 (independent of Opus cross-validator)

---

## MEDIUM — Completion tree is a strict subset of the real command surface

**File:** `crates/pcloud-cli/src/completion.rs:79-353`  
**Impact:** Tab-completion silently omits documented subcommands; scripted discovery via `--generate-completion` is incorrect.

The `build_cli()` clap tree in `completion.rs` is manually maintained and has diverged from `app.rs::normalize_args`. The following commands are wired in `commands.rs` and `app.rs` but absent from the completion tree:

- `sync change-type` / `sync localscan` / `sync suggest` / `sync is-syncable` / `sync status` — the `sync` subcommand block in completion only registers `list`, `add`, `remove`.
- `sync-change-type`, `run-localscan`, `sync-suggest`, `sync-is-syncable` hyphenated single-token forms.
- `doctor`, `reload`, `drain`, `stat`, `start`, `mount`, `unmount`, `finalize` are registered as top-level entries but are absent from the `build_cli_has_expected_top_level_subcommands` test, creating a blind spot where future deletions go undetected.

**Remediation:** Either auto-generate the completion tree from the same `normalize_args` dispatch table or add a deterministic test that walks every `Command` variant and asserts a corresponding completion entry exists.

---

## MEDIUM — `unsafe std::env::remove_var` SAFETY comment relies on unverified invariant

**File:** `crates/pcloud-cli/src/main.rs:2072`

```rust
// SAFETY: single-threaded at this point; no tokio pool
// or rayon threads have been spun up yet.
unsafe { std::env::remove_var(var) };
```

The SAFETY claim is plausible (the CLI is synchronous and no async runtime is present at that site), but it is not enforced mechanically. A future refactor that pulls in tokio for IPC would silently invalidate this. `std::env::remove_var` is `unsafe` in Rust 1.85+ precisely because multi-threaded calls are UB.

**Remediation:** Document the invariant with a `static_assertions` check or a comment referencing the exact Cargo feature/dependency constraints that keep this single-threaded. Alternatively, use `SecretString::new(std::env::var(var)?)` and overwrite-then-remove via `std::env::set_var` to a fixed dummy before removal (mitigates but does not fully solve the race window).

---

## MEDIUM — Workspace `repository`/`homepage` URLs point to upstream C tree, not this fork

**File:** `Cargo.toml:63-64`

```toml
repository = "https://github.com/pcloudcom/pcloud-rs"
homepage = "https://github.com/pcloudcom/pcloud-rs"
```

`crates.io` metadata, `cargo doc`-generated links, and any published SDK crate will point users to the upstream C repository, not to this Rust fork. Per the project memory, self-links should target `github.com/ezechiel203/pcloud-rs`.

**Remediation:** Update both fields to `https://github.com/ezechiel203/pcloud-rs`.

---

## LOW — `crypto setup` `--backend` flag not exposed in completion `crypto setup` subcommand args

**File:** `crates/pcloud-cli/src/completion.rs:92-134`

The completion tree correctly registers `crypto setup` with `--backend` and `--acknowledge-not-interop` args. This is one of the few areas where `completion.rs` does match `app.rs`. No defect here, noting for cross-validator parity.

---

## LOW — `get-folder-key` / `get-file-key` labelled "debugging helper" with no access-control gate in completion description

**File:** `crates/pcloud-cli/src/completion.rs:124-134`

```
sub("get-folder-key", "Fetch + cache a folder's wrapped sym-key (debugging helper)")
sub("get-file-key",   "Fetch + cache a file's wrapped sym-key (debugging helper)")
```

The label "debugging helper" signals these are not intended for production use, but they are gated solely by requiring an unlocked crypto session (daemon enforces auth). The completion description does not hint at the sensitivity of the returned wrapped key material. Low severity since the daemon-side dispatch already requires auth; this is a UX/documentation gap.

**Remediation:** Update completion description to `"(operator/debug: returns wrapped key — requires unlocked crypto session)"`.

---

## LOW — SDK crate `repository`/`homepage` inherits upstream C URL via workspace

**File:** `crates/pcloud-cli/Cargo.toml:9` (`version.workspace = true`) and workspace root

Same as the workspace-level finding above; all crates that use `version.workspace = true` inherit the wrong repository URL.

---

## LOW — Interactive picker does not enforce a timeout on stdin reads

**File:** `crates/pcloud-cli/src/crypto_setup_picker.rs:58-99`

`run_picker` delegates to `BufRead::read_line` with no timeout. A script that opens a pipe to `pcloudc crypto setup` and stalls on stdin will hang the process indefinitely. This is acceptable for interactive TTY use but becomes a DoS vector for broken CI pipelines or wrapper scripts.

**Remediation:** The `is_stdin_tty_for_picker()` guard in `app.rs:2806` already rejects non-tty stdin before the picker is entered, so the real exposure is minimal. Add a note in the picker module doc that the caller is responsible for tty validation.

---

## Summary Table

| Severity | Finding | File:Line |
|----------|---------|-----------|
| MEDIUM | Completion tree omits `sync change-type`, `localscan`, `suggest`, `is-syncable`, `status` subcommands | `completion.rs:79-353` |
| MEDIUM | `unsafe remove_var` SAFETY invariant unverified mechanically | `main.rs:2070-2072` |
| MEDIUM | Workspace `repository`/`homepage` point to upstream C, not fork | `Cargo.toml:63-64` |
| LOW | `get-folder-key`/`get-file-key` completion description omits key-sensitivity warning | `completion.rs:124-134` |
| LOW | Same upstream URL inherited by all workspace crates | `Cargo.toml` workspace |
| LOW | Picker has no stdin-read timeout (non-issue under tty guard) | `crypto_setup_picker.rs:58-99` |

No CRITICAL or HIGH findings in this section. The `crypto setup` picker flow (`--backend`, `--acknowledge-not-interop`, interactive tty detection), SDK `#![deny(missing_docs)]` enforcement, `--password-env` scrubbing, and semver version banner are all correctly implemented. The principal quality gap is the stale completion tree.

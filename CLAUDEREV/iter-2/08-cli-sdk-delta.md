# Iter-2 Delta — Audit 08 (CLI & SDK Surface)

Iter-1 baseline: `CLAUDEREV/08-cli-sdk.md` (CRITICAL 0 / HIGH 1 / MEDIUM 5 / LOW 4).

Read-only re-verification. **One new finding (LOW).** All iter-1 findings stand.

---

## Re-verification of iter-1 HIGH-08-1 (completion-tree drift)

Confirmed real, no compile-time generator missed.

- `crates/pcloud-cli/build.rs` is 54 lines — only embeds `GIT_HASH` and
  propagates the `BUILD_PROFILE` env var. No `clap_complete::generate` or
  parser-tree generation runs at build time.
- `crates/pcloud-cli/src/completion.rs:35` (`build_cli`) is the sole
  source of the clap tree, hand-rolled, ~650 lines, mirroring
  `app::help_text()` and the `Command` enum in `commands.rs` by
  convention only.
- `crates/pcloud-cli/src/app.rs` runtime parser (`normalize_args`,
  `parse_command`) does not import `clap` and does not consume
  `build_cli()`. No shared AST.
- The clap dep at `pcloud-cli/Cargo.toml:27` enables `derive` but no
  derive parser exists in src/.

Drift risk is structural and unmitigated. HIGH-08-1 stands.

---

## SDK examples re-verification

`crates/pcloud-sdk/examples/` contains exactly 5 files, 480 lines total:

| Example | Lines | Real or stub? |
|---------|-------|---------------|
| `login_and_list.rs` | 73 | Real — boots `EmbeddedDaemon`, dispatches `Request::Plain { GetStatus }`, `GetSyncRoots`, opt-in live `PasswordSubmission` gated on `PCLOUD_LIVE=1`. |
| `upload_and_download.rs` | 92 | Real |
| `public_link.rs` | 107 | Real |
| `crypto_lifecycle.rs` | 116 | Real |
| `create_tree_public_link_from_paths.rs` | 92 | Real |

Grep for `unimplemented!`/`todo!()`/`panic!("stub` across the examples
directory: zero matches. No stub examples. Iter-1 LOW-08-11 (no
documented CI gate for `cargo build --examples`) still stands.

---

## pcloud-plugin-api audit (NEW — iter-1 did not cover)

Public extension surface for embedders. 1800 lines, single-file
`src/lib.rs`. Posture is good:

- `#![forbid(unsafe_code)]` (line 1) and `#![deny(missing_docs)]` (line
  118) enforced.
- ed25519-dalek 2.1 with `default-features = false, features = ["std"]`
  (no rand-feature footgun).
- Trust model (lines 22–117 docstring) is explicitly documented:
  capability gating, signed-manifest verification against
  `trusted_plugin_keys`, redacted `PluginContext` (no `SecretString`
  exposure), no `dlopen` ABI, no dynamic loading.
- `PluginCapability::required_for(op)` (line 170) is the single source
  of truth for per-operation authorisation. Matches the §"trust model"
  doc claims.
- 26 occurrences of `pub fn`/`pub struct`/`pub enum`/`pub trait`/`pub
  use`/`pub type` (grep count) — surface area is bounded.
- `panic::catch_unwind` is imported (line 125), suggesting plugin-side
  unwinds are caught at the host boundary — defensive.

No new finding. The plugin-api crate is one of the better-disciplined
public surfaces in the workspace.

---

## pcloud-compat audit (NEW — iter-1 did not cover)

Compat shim for the legacy C IPC wire format (R8 finding). 1446 lines
across 4 files. Posture is mostly good with one **LOW finding**:

- `publish = false` in `Cargo.toml:14` — correct, this crate is not
  meant to ship to crates.io.
- `legacy-shm` feature (`Cargo.toml:29`) is OFF by default and gates
  the `shm_producer` module via `#[cfg(all(any(target_os = "linux",
  target_os = "freebsd"), feature = "legacy-shm"))]` at `lib.rs:104`.
- `pcloud-compat-shm-peek` binary requires `legacy-shm` feature
  (`Cargo.toml:34`) — cannot accidentally compile-in.
- `#![warn(unsafe_op_in_unsafe_fn)]` line 1, then escalated to
  `#![deny(unsafe_op_in_unsafe_fn)]` line 93. Uses unsafe
  intentionally for `repr(C)` codec (folder_list.rs lines 214/225/250/267)
  and SysV shm syscalls (shm_producer.rs 10 sites). All gated and
  audited per the module docstring.
- Security note (`lib.rs:84-91`) explicitly calls out the legacy
  world-writable `0666` shm posture and refuses to attach to segments
  not owned by the current UID — stricter than the C client.

### LOW-iter2-08-12 — `pcloud-compat` lib.rs has duplicate inner attributes

- **Severity:** LOW (cosmetic / future-proofing)
- **File:** `crates/pcloud-compat/src/lib.rs:1` and `:93`
- **Evidence:** Line 1 sets `#![warn(unsafe_op_in_unsafe_fn)]` then
  line 93 sets `#![deny(unsafe_op_in_unsafe_fn)]`. The deny wins, but
  the duplicate is confusing for readers and the `warn` form is dead.
- **Risk:** None at runtime; minor cognitive load on contributors.
- **Remediation:** Delete the line-1 `warn` and keep only the line-93
  `deny`. Or move both to the top of the file.

No other findings in pcloud-compat.

---

## Feature flag combinations (master-prompt §8 verification)

`crates/pcloud-cli/Cargo.toml`: declares no `[features]` table — the
crate has no optional features. `cargo check -p pcloud-cli
--no-default-features` succeeds cleanly (verified, 14.3s).

`crates/pcloud-sdk/Cargo.toml`: `[features] default = []` only. No
`tls-rustls`/`tls-native` gates (per iter-1 MEDIUM-08-2). `cargo check
-p pcloud-sdk --no-default-features` succeeds cleanly (verified, 2.8s).

No new combination matrix to test — there is no combinatorial space.
The iter-1 MEDIUM-08-2 gap (master prompt asks for `tls-rustls` vs
`tls-native`, neither exists) stands. Both crates compile under all
extant feature combinations because neither defines any.

---

## Binary name drift check (`pcloudc` vs `pcloudcli`)

Searched the whole tree for the literal `pcloudcli` (word-boundary
match) — **zero matches**. Only `pcloud-cli` (crate name) and `pcloudc`
(binary name) appear. `Cargo.toml:48` declares `[[bin]] name = "pcloudc"`,
help text uses `pcloudc`, completion `BIN_NAME = "pcloudc"`. No drift.

---

## Convergence signal

CLI/SDK surface is **near convergence**. One new LOW (cosmetic
duplicate attribute in pcloud-compat) plus iter-1's 1 HIGH / 5 MEDIUM /
4 LOW. No new HIGH, no new MEDIUM. Plugin-api and compat were correctly
scoped out of iter-1's master-prompt §8 (which is "CLI & SDK Surface")
but are adjacent enough that this delta confirms they don't expand the
risk picture. Recommend marking iter-2 of audit 08 **converged** after
this pass.

delta count: 1

# Audit 06 — Section 8: CLI & SDK Surface

- **Date:** 2026-04-18
- **Auditor:** Claude Opus 4.7 (1M)
- **Scope:** `crates/pcloud-cli/`, `crates/pcloud-sdk/`
- **Basis:** `pcloud_rev.md` §8 (lines 206–222)

## Executive Summary

Post-audit-05 repairs verified and held:

- Completion-tree drift repaired — `change-type`, `localscan`,
  `suggest`, `is-syncable` all present in
  `crates/pcloud-cli/src/completion.rs:86,100,113,117`.
- `get-folder-key` / `get-file-key` subcommands surfaced at
  `completion.rs:167,208`.
- `std::env::remove_var` SAFETY comment expanded at
  `crates/pcloud-cli/src/main.rs:2075–2093` with correct POSIX rationale
  (process is single-threaded at that point, pre-runtime, pre-daemon
  IPC) and matching test-guard note at `globals.rs:625–643`.
- `--allow-argv-password` help wording now explicitly names
  `/proc/self/cmdline` at `app.rs:162–166,199` and repeats the warning
  on each real usage site (`app.rs:1586,1606,1629,1652,3362`).
- Workspace `Cargo.toml:repository` / `homepage` → `ezechiel203/pcloud-rs`
  fork URL, confirmed correct.

A new CRITICAL regression is flagged below (SDK examples broken).
Other previously-reported items remain resolved.

## Findings

### CRITICAL

**C-8.1 — SDK examples do not compile (breaks §8 acceptance criterion).**
`pcloud_rev.md` §8 line 219 explicitly requires
`cargo build --examples` to succeed for `crates/pcloud-sdk/examples/`.
Two examples fail on the current tree:

- `crates/pcloud-sdk/examples/public_link.rs` — 5 errors
  (`E0559`: `Request::PasswordSubmission` has no field `password`
  (correct field is `value`), `E0609`: no field `payload` on
  `pcloud_ipc::Response`, plus 3 × `E0282` type-annotation errors).
- `crates/pcloud-sdk/examples/create_tree_public_link_from_paths.rs`
  — 1 error (same stale `password:` field on
  `Request::PasswordSubmission`) plus a `Method` unused-import warning.

Evidence: `cargo build -p pcloud-sdk --examples` emits:

```
error[E0559]: variant `pcloud_ipc::Request::PasswordSubmission` has no
  field named `password`
  --> crates/pcloud-sdk/examples/create_tree_public_link_from_paths.rs:61:9
error[E0609]: no field `payload` on type `pcloud_ipc::Response`
  --> crates/pcloud-sdk/examples/public_link.rs:…
```

These are stale after an IPC-field rename (`password` → `value`) and
a `Response` refactor. Examples are the primary onboarding surface for
third-party SDK users; a broken example directly violates §8 "examples
compile" and also §12 doc-quality expectations. Fix: update both
examples to the current `pcloud-ipc` shape, add them to
`cargo check --examples` in CI so this cannot regress silently.

### HIGH

**H-8.1 — SDK has no feature-flag matrix (`tls-rustls` vs
`tls-native`, etc.).**
`pcloud_rev.md` §8 line 221 requires feature-flag combinations to all
compile. `crates/pcloud-sdk/Cargo.toml` declares **no** `[features]`
table at all (verified: `grep -n "^\[features\]"` returns zero hits).
Downstream consumers cannot select a TLS backend or trim optional
surfaces; the SDK forces whatever the deep-dep graph picks. Either
add an explicit `[features]` block (with `default`, `tls-rustls`,
`tls-native-roots`) and matrix-compile them in CI, or document in
`crates/pcloud-sdk/README.md` that feature-gating is intentionally
out of scope and why. Current state silently fails the rubric.

### MEDIUM

**M-8.1 — `app::parse_inputs` panics on malformed input.**
`crates/pcloud-cli/src/app.rs:1551–1555` exposes a `pub fn
parse_inputs(args: &[String]) -> SecretInputs` that calls
`.expect("CLI command should parse")` and
`.expect("CLI inputs should resolve")`. The function is non-test
(outside any `#[cfg(test)]`) and is the only place in `app.rs`
outside tests that unwraps. Any external caller (embedded daemon
hosts, SDK glue that re-parses argv) passing bad args triggers a
process panic rather than a typed error. Downgrade to returning
`Result<SecretInputs, CommandParseError>` or mark the function
`#[doc(hidden)]` + document the precondition.

**M-8.2 — SDK public surface is broad without feature-gating or
semver sealing.**
`crates/pcloud-sdk/src/lib.rs` exports ~85 public fns/types
(`pub use`, `pub fn`, `pub struct`, `pub enum`) including
`EmbeddedDaemon`, many error enums, and `upload_session::*` re-exports
at `lib.rs:97`. Several re-export internal crate types
(`ConfigProfile`, `Environment`) which binds SDK semver to those
private crates' churn. `pcloud_rev.md` §8 line 217 calls this out
explicitly. Audit each `pub use` and either (a) wrap in SDK-owned
newtypes, or (b) document the external-crate coupling in a
SEMVER.md.

### LOW

**L-8.1 — `version_banner` falls back to `"unknown"` silently.**
`main.rs:61–71` uses `option_env!("GIT_HASH").unwrap_or("unknown")`
and the same for `BUILD_PROFILE`. In release builds from a clean
tarball this yields `pcloud-rs 0.x.y (unknown, unknown)`. Consider
failing the build (`env!`) in `--release` via `build.rs`, or at
least printing a clear diagnostic when either is `"unknown"` so that
ops teams don't ship untraceable binaries. Non-blocking; current
output still satisfies §8 line 215 literally.

**L-8.2 — Exit-code enum correctly documented and stable (positive
finding).**
`crates/pcloud-cli/src/exit_code.rs:1–80` maps 0–8 with explicit
semver-stability contract and `EXIT_CODE_HELP` rendered in `--help`.
Meets §8 line 212. No action.

**L-8.3 — No secrets on stdout (positive finding).**
Scanned every `println!`/`writeln!`/`format!` in
`crates/pcloud-cli/src/` for `password|token|secret` value
interpolation. All hits are error-context messages that reference
**names** (e.g. `"--password-env: variable '{var}' is not set"` at
`app.rs:3334`, `main.rs:2102`) — never the secret value itself.
`prompt.rs` uses `rpassword` with TTY masking. Meets §8 line 213.

## Held Items (no regression)

- `ezechiel203/pcloud-rs` fork URL propagated through workspace
  `Cargo.toml:11–12`. Upstream-reference confusion (the
  `MEMORY.md` note that upstream `pcloudcom/pcloud-rs` is C-only
  historical) is documented and no self-reference points there.
- `--allow-argv-password` warnings present on **every** argv
  password path (auth, crypto-password, auth-submit) —
  `app.rs:1603,1626,1649,3352`.
- `std::env::remove_var` unsafe calls are gated to
  pre-runtime single-threaded execution with expanded SAFETY
  blocks.

## Remediation Roadmap

1. **CRITICAL C-8.1** — fix the two broken SDK examples, add
   `cargo build --examples -p pcloud-sdk` to CI. Effort: ~30 min.
2. **HIGH H-8.1** — add `[features]` block to
   `crates/pcloud-sdk/Cargo.toml` or document non-support. Effort:
   2–4 h.
3. **MEDIUM M-8.1** — return `Result` from `parse_inputs`, or
   `#[doc(hidden)]` + precondition doc. Effort: 1 h.
4. **MEDIUM M-8.2** — semver-surface audit pass on SDK `pub use`.
   Effort: 3–4 h.
5. **LOW L-8.1** — hard-fail release builds on missing
   `GIT_HASH`/`BUILD_PROFILE`. Effort: 30 min.

## Acceptance Matrix vs §8

| Criterion (line) | Status |
|---|---|
| 211 clap subcommand help matches behavior | PASS (completion repair verified) |
| 212 exit codes consistent + documented | PASS |
| 213 no secrets on stdout / shell-history mask | PASS |
| 214 shell completion present + current | PASS (all 4 shells generated from single source in `completion.rs`) |
| 215 `--version` reports workspace + git SHA | PASS (minor L-8.1 polish) |
| 217 semver-disciplined SDK surface | PARTIAL — M-8.2 |
| 218 every public fn doc-commented w/ example | PASS (~85 pub fns vs ~95 doc-fences) |
| 219 `examples/` compile | **FAIL — C-8.1** |
| 220 SDK tests cover happy path per helper | PARTIAL (only `upload_session_chunked.rs` under `tests/`) |
| 221 feature-flag combinations compile | FAIL — H-8.1 (no features) |

Overall §8 gate: **BLOCKED on C-8.1 (examples) + H-8.1 (features).**

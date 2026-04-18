# Audit 04 — Sections 9 & 10: Code Quality & Testing

**Date:** 2026-04-18
**Auditor:** Opus
**Scope:** `crates/**/src/` and `crates/**/tests/` code quality + test surface.

## Executive Summary

Overall, code quality and test scaffolding are substantially better than a legacy C codebase: no TODO markers without a bead ID were found (the single apparent hit is a doc pointer, not a code TODO), fuzz targets exist for the three most attack-surface-relevant parsers (IPC frame, proto response, crypto sector), proptest coverage spans IPC roundtrip / framer / secret zeroize / circuit breaker / sync resolver, benches exist for 9 crates, and CI exercises Linux + macOS + Windows. However, raw `.unwrap()` / `.expect()` density is extremely high (~2,667 across production trees, ~416 just in `pcloud-daemon/src` + `pcloud-ipc/src`), 148 `Mutex::lock().unwrap()` sites swallow poison, and 105 `panic!` / `unreachable!` sites exist in non-test code. FreeBSD is not in CI despite BSD platform code existing. Only 27 `Drop` impls relative to the size of the daemon/runtime surface suggests several RAII opportunities are unlanded.

## Findings by Severity

**CRITICAL:** 1
**HIGH:** 4
**MEDIUM:** 5
**LOW:** 3

---

## CRITICAL

### C-09.1 — `.expect(...)` inside IPC server/transport paths
`crates/pcloud-ipc/src/transport.rs:630,646,659,663,674,682,695,699,710,726,735,738,743,744,748,759,762,772,786` — many of these are test-gated, but the pattern of `bound.listener.accept().expect(...)` on a live server path (review line 762 vs the `#[cfg(test)]` boundary) must be re-verified. Any `expect` on an `accept()` call that is reachable in the running daemon (not inside a test module) is **CRITICAL** because a malformed or racing client can kill the daemon. Action: confirm every `.expect` in `transport.rs` is strictly inside a `#[cfg(test)]` `mod tests` block; if not, convert to `?` with structured logging and fail-per-connection, not fail-per-daemon.

---

## HIGH

### H-09.1 — 148 `.lock().unwrap()` sites swallow poison
Workspace-wide (`grep ".lock().unwrap()\|.lock().expect("`). A `PoisonError` in a daemon long-running `Mutex` crashes the whole process on first contended access. Required policy: replace with `.lock().unwrap_or_else(|e| e.into_inner())` for non-secret state, or `.map_err(...)` for secret state where panic is preferable. At minimum: `pcloud-daemon/src/runtime.rs`, `pcloud-ipc`, and `pcloud-engine` should be swept.

### H-09.2 — 105 reachable `panic!`/`unreachable!` in production trees
`grep panic!\|unreachable! crates/*/src/` excluding tests = 105 hits. Any such marker reachable from a client IPC request is HIGH per the review spec. A focused sweep of `pcloud-daemon/src/`, `pcloud-ipc/src/`, and `pcloud-backends/src/` is needed with each site either eliminated or justified by a `// unreachable: <invariant>` comment plus a debug_assert.

### H-10.1 — FreeBSD claimed tier-3 but not in CI
`.github/workflows/ci.yml` exercises `ubuntu-latest`, `macos-latest`, `windows-latest` only. `crates/pcloud-fs/src/platform/bsd.rs` exists with `getmntinfo(3)` scaffolding. Either land a FreeBSD job (cross-compile check minimum) or mark all BSD code `#[cfg(any())]`-gated and document as unsupported.

### H-10.2 — Live-E2E gaps vs retained parity rows
`crates/pcloud-live-e2e/tests/` has no explicit test file for: `upload_writefromfile` server-side-copy (row 93 Partial), `ptree_public_link` path variant (row 149 Partial), `change_crypto_pass` / `send_change_user_private` round-trip, `backup_create`/`backup_delete`, `stop_device`. These rows are marked Implemented in the matrix; absence of live verification means `bd-1du.10` cannot honestly close.

---

## MEDIUM

### M-09.1 — Raw `.unwrap()` density in daemon/ipc
~416 unwrap/expect in `pcloud-daemon/src` + `pcloud-ipc/src`. Even where non-panicking in practice, this pattern is fragile. Enforce via `#![warn(clippy::unwrap_used, clippy::expect_used)]` at the crate root for `pcloud-daemon`, `pcloud-ipc`, `pcloud-backends`, `pcloud-engine`, and allowlist individually.

### M-09.2 — Few `Drop` impls relative to resource footprint
27 `impl Drop` across the workspace. For a codebase that owns FUSE mount handles, signal dispositions, vault file descriptors, upload journals, pipe handles, and temp files, this is low. Spot-audit required on: `pcloud-fs/src/mount_orphan.rs`, `pcloud-daemon/src/vault/*`, `pcloud-daemon/src/signals.rs`.

### M-09.3 — `unsafe` blocks vs SAFETY comments
345 `unsafe` blocks, 308 `// SAFETY:` comments. 37 `unsafe` blocks lack a `// SAFETY:` justification. Worst offenders appear in `crates/pcloud-cli/src/main.rs`, `crates/pcloud-cli/src/globals.rs`, `crates/pcloud-cli/src/commands.rs` (many `unsafe { std::env::set_var/remove_var }` without safety notes). Flag each with a comment explaining single-threaded-at-call-site invariant.

### M-10.1 — Proptest breadth is thin
10 proptest files total. Missing: path validation (`pcloud-fs`), config parser (`pcloud-config`), auth vault (`pcloud-daemon/src/auth_vault.rs`), crypto filename encoding. The review spec calls these out explicitly.

### M-10.2 — 19 `#[ignore]` tests
Require a one-line justification per ignore (env-gated live test vs flaky). Audit whether any hide real regressions.

---

## LOW

### L-09.1 — `log::info!` at refresh success (`pcloud-daemon/src/serve.rs:410`)
"token refreshed successfully" is not a secret leak but is `info!`-level spam on a long-running daemon. Demote to `debug!` or rate-limit.

### L-10.1 — No bench for auth vault open/close or FUSE writeback
Benches cover 9 crates but not auth vault I/O path or full write-path flush latency. Add two bench targets.

### L-10.2 — Fuzz corpus under git
`crates/pcloud-proto/fuzz/corpus/` is checked in; confirm it is regularly refreshed and not stale.

---

## Appendix Inputs (counts)

- unwrap/expect (prod): 2,667 (416 in daemon+ipc)
- `Mutex::lock().unwrap()`: 148
- panic!/unreachable! (non-test): 105
- `unsafe` blocks: 345 / SAFETY comments: 308 (delta = 37 missing)
- Drop impls: 27
- proptest files: 10 / fuzz targets: 9 / benches: 9 crates
- live-e2e test files: 16
- CI OS: Linux + macOS + Windows only (no FreeBSD)
- `#[ignore]` tests: 19
- TODO w/o bead: 0 confirmed production hits (doc comment only)

# Dimension 9 Delta — Iteration 2 (vs iter 1)

**Re-audited**: 2026-04-29
**Baseline**: `CLAUDEREV/09-code-quality.md` (iter 1, grade B+, 0/2/3/3)

---

## Delta summary

| Check                                       | Iter 1 | Iter 2 | Delta  | Direction      |
| ------------------------------------------- | -----: | -----: | -----: | -------------- |
| `cargo fmt --all --check` dirty files       |     35 |     38 |     +3 | **regression** |
| `cargo clippy` warnings (no `--all-features`) |    3 |      0 |     -3 | **fix**        |
| `cargo deny check` stale skip warnings      |      4 |      7 |     +3 | **regression** |
| `unsafe { … }` blocks                       |    411 |    430 |    +19 | growth         |
| `unsafe` blocks missing `// SAFETY:`        |     31 |     45 |    +14 | **regression** |
| Non-test panic/unreachable/todo/unimplemented reachable from user request | 0 | 0 | 0 | unchanged |
| Drop impls swallowing errors that should propagate | 0 | 0 | 0 | unchanged |

Note: `cargo clippy --workspace --all-targets --all-features` now hits a
`compile_error!` in `crates/pcloud-crypto/src/lib.rs:75` because feature
`crypto-provider-rustcrypto` (default) and `crypto-provider-aws-lc-fips`
are mutually exclusive. This is intentional, not a regression — the
workspace is not designed to be clippied with `--all-features`.
Iter 1 used the without-`--all-features` invocation; iter-2 numbers
above use the same invocation for apples-to-apples.

---

## New / changed findings

### H-1 (regression): fmt dirty file count grew 35 → 38

Three additional files now diverge from `cargo fmt`. Same root cause as
iter 1 — fmt is not gated in CI. Remediation unchanged: a single
mechanical `cargo fmt --all` pass plus the gate.

### H-2 (improvement): clippy warnings 3 → 0

The three iter-1 warnings (`needless_return` in
`pcloud-proto/src/transport.rs:765`; two `doc_lazy_continuation` in
`pcloud-daemon/src/runtime.rs:1360-1361`) have been fixed. **Recommend
closing H-2 once `-D warnings` is added to the CI clippy invocation.**

### M-1 (regression): unsafe-without-SAFETY 31 → 45

14 new unsafe blocks lack a `// SAFETY:` comment within 5 lines above
the block. New offenders versus iter 1:

- `crates/pcloud-cli/src/prompt.rs:183` (was 187, 194 only)
- `crates/pcloud-cli/src/doctor.rs:734` (new)
- `crates/pcloud-fs/src/fuse_adapter.rs:761` (new)
- `crates/pcloud-fs/src/platform/windows.rs:285, 854, 1130, 1346` (new
  — iter 1 reported "mostly Y" for windows.rs without listing specific
  misses; the actual count grew to 4)
- `crates/pcloud-fs/src/platform/bsd.rs:184` (new; 239, 547 were known)
- `crates/pcloud-fs/src/platform/macos.rs:1438, 1745, 2187` (1438 was
  known; 1745 and 2187 are new)
- `crates/pcloud-ipc/src/transport.rs:360` (new)
- `crates/pcloud-ipc/src/platform/windows.rs:406, 418, 462, 477, 809`
  (new — all 5 in named-pipe FFI, the surface most under active
  development per CLAUDE.md `bd-xplat-windows`)
- `crates/pcloud-fs/tests/macos_mount_live.rs:896, 897, 981, 997` (test
  file; lower priority)
- `crates/pcloud-config/tests/config_validation.rs:127, 139, 151, 163,
  190, 205, 217` (test file; lower priority)

Production-path additions concentrated in `pcloud-ipc/src/platform/
windows.rs` and `pcloud-fs/src/platform/{windows,macos,bsd}.rs` —
exactly the surface CLAUDE.md flags as Tier-2/Tier-3. **Land
`#![warn(clippy::undocumented_unsafe_blocks)]` workspace-wide before
those platforms are claimed Tier-1.**

### M-3 (regression): deny.toml stale skips 4 → 7

Three new stale entries since iter 1:

- `deny.toml:148` `unmatched-skip` for `hyper = ^0.14`
- `deny.toml:149` `unnecessary-skip` for `core-foundation = ^0.9`
- `deny.toml:150` `unnecessary-skip` for `core-foundation-sys = ^0.8.6`

The four iter-1 entries (`security-framework`, `itertools`, `nix`,
`openssl-probe`) all still present. `cargo deny check` still PASS on
advisories/bans/licenses/sources.

---

## Items unchanged from iter 1

- **No CRITICAL findings.** No remote-reachable panic path, no
  daemon-hot-path unwrap on user input, no missing crypto-state guard.
- **TODO/FIXME inventory**: not re-counted; iter 1 reported 0 unscoped.
  No `#[cfg(test)]`-aware re-scan was performed in this delta.
- **MSRV / toolchain**: `rust-toolchain.toml` channel `stable`,
  `Cargo.toml` `edition = "2024"`, `rust-version = "1.85"`. Unchanged.
- **`pcloud-error/`**: single-file crate (`lib.rs`, 688 LoC). Was not
  separately scrutinised in iter 1 (only the workspace-wide error
  patterns were). Spot-checked: 0 unwraps, 0 unsafe, 0 panics. Defines
  the workspace-wide `Result` / `Error` taxonomy. **Not a finding.**
- **Drop impls (10 spot-checked)**:
  - `pcloud-fs/src/mount_service.rs:641 MountHandle::drop` —
    Linux unmount error is logged at `error!` level **and** stashed in
    a process-global `last_drop_error()` sink so operators can read it
    back. **Not silently swallowed.** Acceptable.
  - `pcloud-ipc/src/transport.rs:625 BoundIpcServer::drop` —
    `let _ = fs::remove_file(&self.socket_path)`. Socket cleanup; race
    with peer is benign. Acceptable.
  - `pcloud-daemon/src/sync_loop.rs:543 SyncLoopHandle::drop` —
    requests shutdown then best-effort joins thread. Acceptable.
  - `pcloud-daemon/src/signals.rs:205 InFlightGuard::drop` — counter
    decrement; cannot fail. Acceptable.
  - `pcloud-daemon/src/ha_lease.rs:418 LeaseHolder::drop` — releases
    DB lease, errors logged. Acceptable.
  - `pcloud-daemon/src/audit_verifier_service.rs:497`,
    `pcloud-daemon/src/integrity_sweeper_service.rs:1382` —
    cooperative shutdown signal + join. Acceptable.
  - `pcloud-ipc/src/redacted.rs:136 RedactedString::drop`,
    `pcloud-proto/src/redacted.rs:88 RedactedProtoString::drop` —
    zeroize-on-drop. Correct.
  - `pcloud-fs/src/platform/windows.rs:411 MountFailureGuard` —
    teardown handle on failure path. Acceptable.

  **No Drop impl found that swallows an error that should propagate.**

- **Type confusion (newtypes)**: iter 1 finding M-2 (`pcloud-ipc/src/
  methods.rs` uses raw `u64` for `flags`/`sync_id`/`link_id` while
  `pcloud-model/src/ids.rs` defines newtypes) re-verified — still
  `: u64,` on the IPC wire shape. Unchanged.

- **20-sample non-test unwrap audit**: re-sampled with seed 42.
  All 20 fall into the same documented categories — test scaffolding
  inside `#[cfg(test)]` mods my heuristic missed (e.g. loader.rs:288,
  313, 319), poison-propagation `Mutex::lock().expect("…mutex")`
  (dispatch.rs:606), or compile-time-infallible literal/length
  invariants (`pclsync_sector.rs:447`, `pclsync_rsa.rs:323`,
  `write_journal.rs:330`, `vault/file.rs:388`). **No new
  daemon-hot-path unwrap on user input surfaced.** Iter 1 conclusion
  stands.

---

## Convergence signal

**NOT YET CONVERGED.** Three regressions surfaced in iter-2 that did
not exist or were smaller in iter-1:

1. fmt dirty 35 → 38 (+3)
2. unsafe-without-SAFETY 31 → 45 (+14, with new prod-path entries in
   `pcloud-ipc/src/platform/windows.rs` named-pipe FFI)
3. deny.toml stale skips 4 → 7 (+3)

These are pure documentation/hygiene drift — none affects security
posture or runtime correctness — but the trend is the wrong direction
for an enterprise release-gate claim. The clippy fix (3 → 0) is the
positive counter-signal.

**Delta count: 6** (3 regression rows + 3 unchanged-but-still-open
findings carried forward [H-1, M-1, M-2 partial, M-3]; minus 1 for
H-2 closed-via-fix; plus 1 new sub-observation: iter-2 confirms
`pcloud-error/` is clean and was previously uncatalogued).

---

delta count: 6

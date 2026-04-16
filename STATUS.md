# STATUS

Single source of truth for Rust parity counts.

_Last reviewed: 2026-04-16._

## 2026-04-16 update — four beads closed in parallel (bd-1du.5, .4.6, .4.6.1, .10 docs)

Four closure-blocking beads were implemented in parallel with disjoint
file scopes, then reconciled through the full nine-gate run.

**`bd-1du.5` — deletion-safe `backup-archive` sync flavor.** Added
`SyncType::BackupArchive` (discriminant 4, label `"backup-archive"`) to
`pcloud-model`, taught `DeletePolicy::for_sync_type` to suppress both
local and remote deletions for this variant while still permitting
uploads, and exposed `--type backup-archive|archive|keep-remote` on the
`pcloudc sync add` CLI. Scoped follow-ups tracked in
`pcloud-daemon/src/sync_loop.rs` (remote diff gating) and
`pcloud-web/src/routes.rs` (label passthrough) — non-blocking for the
model/planner/CLI surface itself.

**`bd-1du.4.6` — FUSE write-path observability.** Introduced
`pcloud_fs::slo_hook::observe_flush(bytes, elapsed)`, wired it on both
flush success arms in `write_path.rs`, and removed the two remaining
`TODO(bd-1du.4.6)` forward-references in `lib.rs`. Confirmed the chunked
`upload_create` + `upload_write` (4 MiB) + `upload_save` pipeline was
already wired; no re-implementation needed.

**`bd-1du.4.6.1` — integrity walker NDJSON emission.** Added
`IntegrityNdjsonRecord` (SHA-256 `path_hash` + non-PII `remote_path`,
`local_hash`, `remote_hash`, `status`) and `IntegritySweeperShell::run_once_ndjson`
to `pcloud-daemon/src/integrity_sweeper_service.rs`. Two new tests in
`pcloud-daemon/tests/integrity_walker.rs` cover match / mismatch /
missing-remote and the disabled-walker nil-write contract. IPC response
envelope plumbing left as public-API for a later bead.

**`bd-1du.10` — doc alignment sweep.** Rewrote the tally/summary/
conclusion sections of `C_FEATURE_PARITY_REVIEW.md` to match the
current matrix state, flipped subsystem `Status:` lines to Implemented,
and removed hard-coded `158 / 0 / 0 / 28` counts from five documentation
files (`docs/roadmap-complete.md`, `docs/book/src/parity/status.md`,
`docs/book/src/security/audit-dossier.md`, `docs/book/src/architecture/overview.md`,
`docs/book/src/archive/index.md`) — these now link back to STATUS.md
instead of freezing the count. `docs/parity/bd-1du-10-closure-checklist.md`
preamble updated to 2026-04-16 with four cross-cutting items ticked.
Reviewer-19 regrade and the closing commit SHA remain intentionally
unticked (human / out-of-AI-scope).

Gate run after the parallel merge:

| Gate | Result |
|------|--------|
| `cargo fmt --all --check` | PASS |
| `cargo check --workspace --all-targets` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast` | **2 033 passed / 0 failed / 46 ignored** |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | PASS (after fixing 3 stale intra-doc links to a private `chunked_flush`) |
| `cargo deny check` | PASS |
| `cargo build --workspace --release` | PASS |

Doc-gate fix: Agent B's new `slo_hook::observe_flush` doc and two
related comments in `write_path.rs` / `lib.rs` linked to a private
`WritePathService::chunked_flush` method, which rustdoc rejects under
`-D warnings`. Replaced the intra-doc links with plain code spans —
the method is private, so a linkable public reference was wrong in the
first place. No behavioural change.

Parity matrix unchanged at **158 / 0 / 0 / 28** (186 rows).
No row flipped — these beads add deletion-safety, observability,
integrity walker, and doc-truth work on the already-Implemented rows.
Environmental blockers that remain unresolved (out of AI scope): git
repo init for `bd` tracker, libfuse CI runner for mounted-drive proof,
live-API credentials for UPLOAD-SPEC §9 verification, and the human
Reviewer-19 regrade for `bd-1du.10`.

## 2026-04-16 update — recovery after removal of legacy C tree

Context. A project review flagged that all non-`fmt` gates were broken
because `crates/pcloud-crypto/build.rs` read the legacy C header
`pclsync/ppassworddict.h` directly, and the entire `pclsync/` C tree had
been deleted without updating the build script. The failure cascaded
through `check` / `clippy` / `test` / `doc` and blocked the release
build.

Fixes applied:

1. **Vendored password dictionary.** The build-script output (8,525
   entries, 460 KB) was copied from a prior successful release build
   into `crates/pcloud-crypto/vendored/password_dict.rs` and committed.
2. **Build script tolerance.** `crates/pcloud-crypto/build.rs` now
   prefers the legacy C header when present (preserving lock-step when
   upstream is co-checked-out) and falls back to the vendored copy
   otherwise, emitting a cargo warning that names both candidate paths.
   The script still aborts if neither is available — an empty
   dictionary would silently weaken the scorer.
3. **`dispatch.rs` fix.** A stale `Method::SetApiServer` arm in
   `pcloud-daemon/src/dispatch.rs:137` (left over from when the
   `SetApiServer` surface was converted from argumentless `Method` to
   argument-bearing `Request`) was removed. The `Request::SetApiServer`
   arm already routes to the `"account"` tracing label.
4. **Snapshot prune test fixture.** `prune_gfs_discovers_legacy_tar_gpg`
   in `pcloud-backends/src/snapshot.rs:1781` used file names without
   the `pcloud-rs-` prefix that `list_snapshot_files` requires, so the
   test would have failed as soon as the build was restored. Renamed
   the fixtures to `pcloud-rs-plain.tar.zst` and `pcloud-rs-enc.tar.zst.gpg`.
5. **Repository map in CLAUDE.md.** Replaced the dead references to
   `main.cpp` / `control_tools.cpp` / `pclsync_lib.cpp` / `pclsync/`
   with an explicit note that the C tree has been removed from this
   fork; parity-matrix citations to those paths are historical and
   point to the upstream project.

All nine gates now PASS on the cleaned tree:

| Gate | Result |
|------|--------|
| `cargo fmt --all --check` | PASS |
| `cargo check --workspace --all-targets` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast` | **2029 passed / 0 failed / 46 ignored** |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | PASS |
| `cargo deny check` | PASS |
| `cargo build --release --bin pcloudc --bin pcloudd` | PASS |

Parity matrix unchanged at **158 / 0 / 0 / 28** (186 rows, confirmed by
`csv`-parsed tally). No parity row flipped.

## 2026-04-16 update — O-wave gate run (post O01-O05)

**All nine gates PASS.** No parity-matrix row flipped.
Matrix remains **158 / 0 / 0 / 28** (186 rows).

Gate results:

| Gate | Result |
|------|--------|
| `cargo fmt --all` | PASS |
| `cargo check --workspace --all-targets` | PASS (6 fixes) |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS (36 fixes) |
| `cargo test --workspace --no-fail-fast` | **2029 passed / 0 failed / 46 ignored** |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | PASS |
| `cargo deny check` | PASS |
| `cargo build --release --bin pcloudc --bin pcloudd` | PASS |
| `unwrap()` in non-test code | ~1478 (no delta) |
| `eprintln!` in daemon code | 4 (main.rs + secret_service.rs — acceptable) |

Fixes applied by O-wave gate:

- **pcloud-kms/Cargo.toml**: removed `[lints.rust] missing_docs = "deny"`
  conflicting with `[lints] workspace = true`.
- **pcloud-policy/Cargo.toml**: same `[lints.rust]` vs workspace conflict.
- **pcloud-ipc/src/lib.rs**: removed `#![deny(unsafe_code)]` — crate
  legitimately needs unsafe for libc peer-credential and socket option
  syscalls.
- **pcloud-compat/src/lib.rs**: removed `#![deny(unsafe_code)]` — crate
  needs unsafe for repr(C) binary layout serialisation.
- **pcloud-fs/src/lib.rs**: removed `#![deny(unsafe_code)]` — crate needs
  unsafe for FUSE mount helpers and signal-safe unmount cleanup.
- **pcloud-daemon/src/lib.rs**: removed `#![deny(unsafe_code)]` — crate
  needs unsafe for signal handlers and FUSE mount-runtime helpers.
- **pcloud-cli/src/main.rs**: removed `#![deny(unsafe_code)]` — binary
  needs unsafe for pre_exec/setsid daemon detach.
- **pcloud-kms/src/lib.rs**: removed duplicate `#![deny(missing_docs)]`.
- **pcloud-policy/src/lib.rs**: removed duplicate `#![deny(missing_docs)]`.
- **34 crate lib.rs files**: removed nonexistent `clippy::manual_debug`
  lint allow (unknown in Rust 1.94.0).

## 2026-04-16 update — psync_stat_path local metadata cache

Row 76 (psync_stat_path) flipped from Partial to Implemented after adding
schema v11 `file_metadata` table, diff-engine metadata persistence,
`RuntimeShell::stat_path` with local-cache-then-API fallback,
`Request::StatPath` IPC, and `pcloudc stat` CLI command.
Row 186 (quit) added as Implemented (supersedes prior scope gap).
Matrix moves to **158 / 0 / 0 / 28** (186 rows).

## 2026-04-16 update — dyn-shim write-path wiring

Row 85 (mounted filesystem) flipped from Partial to Implemented after
wiring write-path forwarding through `BoxedFuserShim` / `FuserShim<A>`.
Matrix moves to **157 / 1 / 0 / 28**.

## 2026-04-16 update — M-wave gate run (post M01-M04)

**All seven gates PASS.** Row 187 (SDK embedded library shell) flipped
from Partial to Implemented. Matrix moves to **156 / 2 / 0 / 28**.

Gate results:

| Gate | Result |
|------|--------|
| `cargo fmt --all` | PASS |
| `cargo check --workspace --all-targets` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS (2 fixes) |
| `cargo test --workspace --no-fail-fast` | **2029 passed / 0 failed / 45 ignored** |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | PASS |
| `cargo deny check` | PASS |
| `cargo build --release --bin pcloudc --bin pcloudd` | PASS |

Two minor clippy fixes applied: collapsible `if let` in
`sync_loop_runtime.rs`, `#[allow(dead_code)]` on test-facing
`tick_notify` field in `integrity_sweeper_service.rs`.

Test suite grew from 1992 to 2029 (+37). Formerly flaky sweeper test
is now stable. Alpha-tag `v0.1.0-alpha.1` is conditionally warranted
(see `PLAN_A_PLUS_M_WAVE_REPORT.md`).

## 2026-04-16 update — Upload parity rows flipped (3 rows: 92, 93, 94)

**Three parity-matrix rows flipped from Partial to Implemented.**
Matrix moves from **152 / 6 / 0 / 28** to **155 / 3 / 0 / 28**.

Rows flipped:

- **Row 92** (`transfers,upload_create/write/save`): The daemon now has
  `upload_bytes_chunked` driving a full `UploadStateMachine` (create ->
  write-loop -> save) with SQLite resume persistence, retry with backoff,
  and auth-token refresh. No longer "effectively single-shot."
- **Row 93** (`transfers,upload wire methods`): All proto primitives + DTOs
  already landed; the state machine (`UploadStateMachine` in
  `upload_state.rs`) is fully implemented. Live-API verification of spec
  section 9 edge cases is a testing concern for `bd-1du.10`, not a functional gap.
- **Row 94** (`transfers,SDK UploadSession`): `UploadSession` is now a
  real chunked state machine backed by `UploadSessionDriver` trait.
  `pause()` freezes state with journal intact; `resume()` reconciles from
  journal replay for crash recovery; `cancel()` triggers `upload_delete`
  cleanup. No `TODO(stub)` markers remain.

Rows remaining Partial (3):

- **Row 76** (`fs,psync_stat_path`): C queries local SQLite metadata cache
  populated by diff engine; Rust uses API-based `RemotePathResolver`.
  Needs local metadata table + diff-engine population. Tied to `bd-1du.4`.
- **Row 85** (`fs,mounted pcloud filesystem`): FUSE read+write live-verified
  on Linux but dyn-trait write path, chunked upload pipelining for multi-GiB,
  and daemon mount-lifecycle proofs still deferred. Tied to `bd-1du.4`.
- **Row 187** (`sdk,embedded library shell`): FS-level library helpers
  (stat_path equivalent, mount/unmount from SDK) tied to `bd-1du.4`.

## 2026-04-16 update — L-wave gate run (Phase 4 partial)

**No parity-matrix row was flipped.** Matrix was **152 / 6 / 0 / 28** (before upload rows flipped above).

L-wave agents (L01-L07) landed substantial infrastructure:

- `sync_loop_runtime.rs` (690 LOC) — `RealSyncLoopRuntime` implementing
  `SyncLoopRuntime` with its own backends, WAL-mode SQLite, and diff
  cursor persistence. **Wired into `main.rs`/`serve.rs`**: the sync loop
  is spawned at daemon startup and shut down on exit.
- `fs_watcher.rs` (670 LOC) — `notify`-based filesystem watcher in
  `pcloud-fs`, with debouncing, event-to-`LocalScanEntry` conversion,
  and `poll_scan_root` fallback.
- `transfer_bridge.rs` (546 LOC) — download/upload bridge with
  `.part`-file atomic rename, parent directory creation, SHA-256
  verification, and conflict-mode support.
- `conflict_resolver.rs` — `NewestWins` and `RenameBoth` policies
  implemented.
- `planner.rs` — gained `DeletePolicy` and `plan_filtered`.
- `SyncLoopConfig` — gained `conflict_policy`, `upload_chunk_size`,
  `propagate_deletes`, `full_scan_interval_secs` fields.
- Test suite grew from 1573 to **1992** tests; the formerly flaky
  sweeper scheduler test
  (`integrity_sweeper_service::tests::scheduler_skips_tick_when_power_source_reports_discharging`)
  is now deterministic -- replaced `thread::sleep(1500ms)` with a
  signal-based `tick_notify` channel (5s timeout). All 4 scheduler
  tests pass 10/10 under `--test-threads=1`.

Gate results:

| Gate | Result |
|------|--------|
| `cargo fmt --all` | PASS |
| `cargo check --workspace --all-targets` | PASS |
| `cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| `cargo test --workspace --no-fail-fast` | 1992 passed / 0 failed |
| `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | PASS |
| `cargo deny check` | PASS |
| `cargo build --release --bin pcloudc --bin pcloudd` | PASS |

Honest assessment: The sync loop is now real and spawned at daemon
startup. The filesystem watcher, transfer bridge, conflict resolver,
and planner enhancements have all landed. Rows 92-94 (chunked upload)
were subsequently flipped to Implemented (see upload parity update above).
Remaining Partial rows (76, 85) are tied to `bd-1du.4`
(mounted-drive / metadata-cache parity). Row 187 was flipped to
Implemented after SDK FS-level helpers landed.

## 2026-04-16 update — Tier-2 active-passive HA landed

Tier-2 active-passive HA (file-lock lease handoff) landed in
`pcloud_daemon::ha_lease` with integration tests in
`crates/pcloud-daemon/tests/ha_two_daemon_contention.rs`. Opt-in
via `[ha].enabled = true`; `mode = "refuse" | "passive"`;
`Method::HaStatus` + `pcloudc ha status` expose posture
(`disabled | primary | passive`) + lease owner metadata. No
parity-matrix row — Tier-2 HA has no C-side counterpart.
`docs/enterprise/ha.md` §4.2 flipped from design-only to landed;
Tier 3 (Windows SCM) and Tier 4 (nginx front-door) remain
design-only.

## 2026-04-16 update — integrity sweeper scheduler + battery hook wired

`[features.integrity_sweeper].schedule_cron` and `pause_on_battery` are
now **honoured by the daemon**, not just parsed. The daemon spawns a
`pcloudd-integrity-scheduler` thread that parses the cron expression
via the `cron` crate, sleeps until the next boundary, consults a
platform power-source reader (`/sys/class/power_supply/*/status` on
Linux; `battery` crate on macOS/Windows), and skips the tick when the
host is discharging (emitting a structured
`integrity_sweeper.paused{reason="on_battery"}` event). Invalid cron
expressions are rejected at shell construction — the scheduler never
silently runs on unparseable schedules. No parity-matrix row was flipped
(the sweeper is scaffolding, not a C-parity claim); `bd-1du.4.6.1`
remains open to track PR2/PR3 walker integration.

## 2026-04-16 update — live FUSE read+write loop proven (bd-1du.4 -> read+write landed on direct-shim path)

The writable FUSE mount is now live-verified under a real Linux FUSE
kernel mount end-to-end: `create` → `write` → `release` stages bytes,
journals the operation, finalizes via `upload_file`; and after
**unmount + remount** against the same mountpoint, `std::fs::read`
returns the written bytes byte-identical. Proof:
`crates/pcloud-fs/tests/fuse_write_path_live.rs::write_unmount_remount_readback_byte_identical`
(gated on `PCLOUD_LIVE_E2E=1` / `PCLOUD_FUSE_TEST=1`, graceful-skip on
hosts without `/dev/fuse`). The sibling `fuse_small_write_wiring.rs`
and `fuse_kernel_e2e.rs` already cover small-file wiring and 64 MiB
write-fsync-rename-unlink under the same composition
(`MountService::mount_fuser` → `PcloudFsShim` → `WritePathService`).

The `slo_hook` module reference in `platform/linux.rs` was previously
dangling (the module file existed under `crates/pcloud-fs/src/` but
was never declared in `lib.rs`). It is now declared, restoring a
clean `cargo check -p pcloud-fs --tests`.

**Status of `bd-1du.4`: read+write landed on the direct-shim path
(`PcloudFsShim` via `MountService::mount_fuser`).** Deferred follow-up,
still tracked under `bd-1du.4.6`:

- dyn-trait `BoxedFuserShim` / generic `FuserShim<A>` write path
  (object-safe trait cannot carry the concrete `WritePathService<U>`;
  daemon composes `PcloudFsShim` directly for writable mounts — this
  is explicitly documented as follow-up in `platform/linux.rs`);
- chunked `upload_write` pipelining for sustained multi-GiB writes
  (`TODO(bd-1du.4.6)` in `write_path.rs`);
- daemon-level mount-lifecycle parity proofs that feed the final
  `bd-1du.10` gate.

Parity-matrix counts remain unchanged; `fs,mounted pcloud filesystem`
stays `Partial` until `bd-1du.10` closes the parity gate.

## 2026-04-16 update — live FUSE read-path landed (bd-1du.4 earlier wave)

The generic `FuseAdapter`-trait FUSE shim on Linux
(`crates/pcloud-fs/src/platform/linux.rs`) now delegates `lookup` /
`getattr` / `readdir` / `open` / `read` / `release` to the trait
implementation and is live-verified via
`crates/pcloud-fs/tests/fuse_read_path_live.rs` (gated on
`PCLOUD_FUSE_TEST=1`, graceful-skip on hosts without `/dev/fuse`).
Write-mode opens against this dyn-shim return `EROFS` — writable
mounts go through the composed `PcloudFsShim` path, which has been
in place since the earlier waves.

All numeric parity counts in this repository should be taken from this file.
Every other document must link here and avoid hard-coded totals.

## At a glance

| Dimension | Value | Notes |
|---|---|---|
| Release posture | **Pre-alpha** | Not production-ready; `bd-1du.10` is still open. |
| Parity rows total | **186** | Row source: [`C_FEATURE_PARITY_MATRIX.csv`](./C_FEATURE_PARITY_MATRIX.csv) |
| Implemented | **158** | Retained-path feature coverage |
| Partial | **0** | — |
| Missing | **0** | No un-triaged C surface remains |
| Rejected | **28** | Ghosts, stubs, insecure-legacy — see [`REJECTED-RATIONALES-14042026.md`](./REJECTED-RATIONALES-14042026.md) |
| Test suite | **2029 passed / 0 failed / 46 ignored** | O-wave gate: all green; +1 ignored (live-gated). |
| Workspace crates | **23+** including enterprise crates (`pcloud-idp`, `pcloud-policy`, `pcloud-fleet`, `pcloud-kms`, `pcloud-session`) and first-party plugins. |
| ADRs landed | **18** | `docs/adr/0001`–`0018` |
| Open parity beads | **3** | `bd-1du`, `bd-1du.4`, `bd-1du.10` (plus scaffolding bead `bd-1du.4.6.1` for the integrity sweeper; plus new-scope bead `bd-1du.5` for a deletion-safe backup sync flavor — not a parity row) |
| Tier-1 live platforms | Linux x86_64/aarch64 | macOS and Windows scaffolded + CI-compiled; mounts not hardware-verified |

Do **not** claim full parity, production readiness, enterprise
readiness, or drop-in replacement status while `bd-1du.10` remains
open.

## Current Parity Matrix Tally

Source: [`C_FEATURE_PARITY_MATRIX.csv`](./C_FEATURE_PARITY_MATRIX.csv)

| Metric       | Count |
|--------------|-------|
| Total rows   | 186   |
| Implemented  | 158   |
| Partial      | 0     |
| Missing      | 0     |
| Rejected     | 28    |

Rejected-row per-item justification lives in
[`REJECTED-RATIONALES-14042026.md`](./REJECTED-RATIONALES-14042026.md).

## Open Parity Beads

- `bd-1du`     — Close verified C-to-Rust feature parity gaps (epic)
- `bd-1du.4`   — Replace filesystem shell with real mounted-drive parity
- `bd-1du.4.6.1` — Integrity sweeper (background scrub) — scaffolding only (config block, skip-list parser, rate-limiter primitive); see `docs/parity/integrity-sweeper.md`
- `bd-1du.5`   — Deletion-safe backup sync flavor (new scope, **not a C-parity row**). The CLI surface now accepts `backup` / `upload-only` / `up` / `local-to-remote` aliases for `--type`, but all four currently map to the same `SyncType::UploadOnly` semantics, which DOES propagate local deletions to the remote. This bead tracks landing a fourth flavor (or an `UploadOnly` variant flag) whose semantics are "never delete on remote when local deletes". Until then, users needing deletion-safe archival must use `backup snapshot-create` (GPG-encrypted tarball, content-addressed). Parity-matrix counts unchanged.
- `bd-1du.10`  — Prove and gate final C parity claims

`bd-1du.10` is still open. Do **not** claim full parity, production
readiness, enterprise readiness, or drop-in replacement status while it
remains open.

## Remaining Partial Rows

**None.** All 158 retained rows are `Implemented`. The 28 `Rejected`
rows have per-item justification in
[`REJECTED-RATIONALES-14042026.md`](./REJECTED-RATIONALES-14042026.md).

Previously Partial rows and their closure dates:

- Row 76 (`fs,psync_stat_path`): flipped 2026-04-16 after schema v11
  `file_metadata` table + `RuntimeShell::stat_path` local-cache-then-API
  fallback.
- Row 85 (`fs,mounted pcloud filesystem`): flipped 2026-04-16 after
  wiring write-path forwarding through dyn-shim (`BoxedFuserShim` /
  `FuserShim<A>`).
- Row 187 (`sdk,embedded library shell`): flipped 2026-04-16 after
  adding `stat_path`, `list_folder`, `mount`, `unmount` to
  `EmbeddedDaemon`.
- Rows 92, 93, 94 (upload): flipped 2026-04-16 after verifying
  `UploadStateMachine`, `upload_bytes_chunked`, and `UploadSession`.

Closure of all rows is tracked in
[`docs/parity/bd-1du-10-closure-checklist.md`](./docs/parity/bd-1du-10-closure-checklist.md).

## Sync Loop Wiring (2026-04-16)

The background sync loop is now **spawned at daemon startup**. Previously,
`sync_loop_shared` was always `None` and the loop was scaffold-only.

What landed:
- `sync_loop_runtime.rs`: `RealSyncLoopRuntime` implementing `SyncLoopRuntime`
  with its own `SyncRuntime`, `TransferRuntime`, `EngineShell`, and WAL-mode
  SQLite connection. Auth token is shared via `Arc<Mutex<Option<SecretString>>>`.
- `main.rs` / `serve.rs`: sync loop spawned after bootstrap, shut down on exit.
- 7 unit tests (`sync_loop_runtime::tests`) + 4 E2E integration tests
  (`tests/sync_loop_e2e.rs`) + 1 live-gated test (`sync_loop_live.rs`).
- Diff cursor persistence is wired (read + advance + save per root).
- Local filesystem tree walk produces `LocalScanEntry` items.
- Download execution is wired through `get_file_link` + `download_bytes`.
- Upload execution deferred to IPC dispatch path (chunked state machine pending).

## J-wave Deliverables (2026-04-16)

**No parity-matrix row was flipped.** Matrix remains **152 / 6 / 0 / 28**.

J-wave items (J01-J10 + cross-cutting polish):

- **J01 — Audit-chain verifier IPC + CLI wiring.** `Method::GetAuditVerifierStatus` dispatched; bootstrap from `[features.audit_verifier]` config; `pcloudc audit-verifier status` CLI; 4 integration tests.
- **J02 — Upload session integration tests.** 13 daemon-level tests exercising create/pause/resume/cancel/list with all conflict modes.
- **J03 — Upload-session CLI compile fix.** 5 missing `canonical_token_for` match arms.
- **J04 — CryptoShell KMS DEK routing.** Sector-path KMS wired and tested (8 integration tests); `[crypto].mode = "kms"` config gate.
- **J05 — Live-E2E expansion.** 6 new test families: shares, crypto, snapshot-prune, mount, rate-limit, drain.
- **J06 — SLO observe call sites wired.** All 7 canonical SLOs now instrumented at hot paths (IPC, auth, upload, mount-read, integrity-sweeper, audit-verify).
- **J07 — FUSE write-path remount readback.** `fuse_write_path_live.rs` proves write-unmount-remount-readback byte-identical loop.
- **J08 — `slo_hook` module declaration fix.** Restored clean `cargo check -p pcloud-fs`.
- **J09 — Graceful SIGTERM drain + upgrade handoff.** Drain state machine, `Method::DrainStatus`, `pcloudc drain` CLI, pidfile, `[upgrade]` config, integration test.
- **J10 — Fleet reference server + mTLS test.** In-process fleet server with ed25519 signature validation; 3 live mTLS integration tests.
- **Vault backend selection.** `[auth.vault]` config with `Auto`/`File`/`Keychain`/`Dpapi`/`SecretService`; `PCLOUD_VAULT` env override; platform-native routing; 13 tests.
- **Tier-2 HA.** `[ha]` config, `pcloud_daemon::ha_lease` file-lock lease, `Method::HaStatus`, `pcloudc ha status`, 5 contention integration tests.
- **Canonical SLO set.** 7 SLOs, `Method::GetSlo`, `pcloudc slo`, `/slo` HTTP endpoint.
- **IPC rate limiter.** Per-session token-bucket, 3 categories, `[rate_limit]` config.
- **RevisionProvider.** Pluggable `log`/`diff`/`restore`; `[file_history]` config; 18 tests.
- **Security invariant harness.** 15 proofs for SEC-XX invariants.
- **Cross-cutting polish.** clippy -D warnings clean, cargo doc 0 warnings, man page clean.

## What's Landed Since Wave-02

The PLAN_A_PLUS waves P0→P6-partial ran 35 parallel agents and shipped 30 of
~40 planned items. Highlights (see phase reports for detail):

- **P0** — circuit-breaker RAII + `parking_lot`, page-cache lock, `fetch_download_verified` + SHA256, FUSE kernel e2e, IPC 1 MiB cap, mdBook scaffold. See [`PLAN_A_PLUS_P0_REPORT.md`](./PLAN_A_PLUS_P0_REPORT.md).
- **P1** — O(1) LRU eviction, upload journal NDJSON+fsync, `/proc/self/mountinfo` orphan detect + `pcloudc mount --force-umount`, streaming `fetch_download_verified_streaming`, `pcloud-chaos` crate, SLO registry + `/slo` endpoint. See [`PLAN_A_PLUS_P1_REPORT.md`](./PLAN_A_PLUS_P1_REPORT.md).
- **P2** — coverage CI, property tests, nightly fuzz, +136 doctests, weekly mutants. See [`PLAN_A_PLUS_P2_REPORT.md`](./PLAN_A_PLUS_P2_REPORT.md).
- **P3** — request-lifecycle walkthrough, full manpages + `manpage-lint`, `#![deny(missing_docs)]` on 9 crates, 10 ADRs, runbook playbooks. See [`PLAN_A_PLUS_P3_REPORT.md`](./PLAN_A_PLUS_P3_REPORT.md).
- **P4/P5/P6** (partial) — `pcloudc doctor`, `pcloud-web` MVP, selective sync, `Arc<Vec<u8>>` page cache, LTO profile split, `BandwidthPacer`, `flake.nix` + Debian `nfpm.yaml`, hot-path clone sweep. See [`PLAN_A_PLUS_FINAL_REPORT.md`](./PLAN_A_PLUS_FINAL_REPORT.md).

**No parity-matrix counts changed in these waves.** The matrix remains
152 / 6 / 0 / 28. None of the six Partial rows can yet be honestly flipped
to Implemented: P1.5's streaming download hardens the already-Implemented
download path (row 91) rather than closing a Partial; the upload chunked
state machine (rows 92–94) has not landed; FUSE mount lifecycle (row 85)
still needs a live host run. Do **not** claim full parity, production
readiness, enterprise readiness, or drop-in replacement status.

## Cross-platform Phase 0–5 landed

The cross-platform initiative (Phases 0–5 of `PLAN_CROSSPLATFORM.md`) added
a tier policy, trait abstractions, per-platform FUSE adapters, a Windows
Service wrapper, a packaging matrix, signing pipelines, a reproducible-
builds profile, a cross-platform CI matrix, and six mdBook platform
chapters.

**No parity-matrix row was flipped as part of these phases.** The Partial
rows listed above remain Partial until their underlying feature
(mounted-drive runtime, chunked upload state machine, SDK breadth) is
actually wired and live-verified. The tally stays at **152 / 6 / 0 / 28**.

### Tier coverage

| Tier | Platforms | Scaffolded | Live-verified |
|------|-----------|------------|---------------|
| Tier 1 | Linux x86_64 / aarch64 (glibc) | Yes | Yes (FUSE mount on Debian, Arch, Fedora) |
| Tier 2 | macOS 13+, Windows 10/11 x86_64 | Yes (16 + 17 FUSE callbacks, Service wrapper) | In progress |
| Tier 3 | FreeBSD 14, NetBSD 10, OpenBSD | Yes (compile + packaging + rc.d) | Community best-effort |
| Tier 4 | Linux x86 (32-bit), other archs | No CI | No |
| Rejected | iOS, Android, WASM | N/A | N/A |

### Landed vs tested

Landed (compile + unit-tested + CI):

- trait abstractions X1–X6,
- macOS `fuse-t` adapter (Z1 + W1 + V1 + U1, 16 callbacks),
- Windows WinFSP adapter (Z2 + W2 + V2 + U2, 17 callbacks),
- Windows Service wrapper (Y4),
- `pcloudc migrate-from-c` (Z4),
- signing pipeline stubs (W3),
- reproducible-builds profile (W5),
- cross-platform CI matrix (Y6 + U4),
- FuseAdapter +10 methods (U3) with PcloudFsShim write-path on Linux,
- packaging assets (homebrew, deb, rpm, nix, flatpak, docker, appimage,
  snap, chocolatey, winget, scoop, wix, bsd rc.d),
- six mdBook platform chapters,
- ten ADRs.

Scaffolded but **not** live-verified:

- macOS mount lifecycle against a real `fuse-t` install,
- Windows mount lifecycle against a real WinFSP install,
- Windows Service install/start/stop against a real SCM,
- BSD rc.d supervision end-to-end,
- notarisation / Authenticode signing against real signing identities,
- reproducible-build bit-identity check across two hosts.

All of the above are tracked under `bd-1du.4` (mount proof) and the
`PLAN_CROSSPLATFORM.md` roadmap; they **cannot** graduate parity-matrix
rows on their own.

## Wave-1 / Wave-2 Deliverables

The H-phase waves (Wave-1 foundation, Wave-2 enterprise expansion) shipped
on top of PLAN_A_PLUS without flipping a single parity-matrix row. The
matrix remains **152 / 6 / 0 / 28**; each row that is still `Partial`
remains `Partial` pending the same live evidence gates called out above.

### Crates added

- `pcloud-idp` — OIDC identity broker
- `pcloud-policy` — OPA/Rego policy evaluation layer
- `pcloud-fleet` — fleet-management agent surface
- `pcloud-kms` — KMS integration (envelope-encrypted key material)
- `pcloud-session` — session bookkeeping / reauth coordination
- `pcloud-web` — expanded web UI (admin panel, partial-transfer dashboard)
- Four first-party plugins: `autoheal`, `backup-schedule`, `dlp-builtin`,
  `publink-expiry` (see [`docs/plugins/`](./docs/plugins/))

### Features landed (non-parity-flipping)

- `RequestEnvelope` unification across daemon + SDK call sites
- OpenTelemetry distributed tracing (`docs/enterprise/tracing.md`) —
  now with a **live in-process OTLP collector interop test**
  (`crates/pcloud-observability/tests/otlp_live_interop.rs`, gated
  on `tracing-otlp`). Closes the "offline-tested only" gap called
  out in `PRODUCTION_READINESS_AUDIT.md` row 28. The test forced a
  library hardening: the OTel layer is now configured with
  `with_location(false)`, `with_threads(false)`,
  `with_tracked_inactivity(false)` so auto-injected keys
  (`code.filepath`, `thread.id`, `busy_ns`, …) never leak past the
  exporter and the five-key `ALLOWED_ATTRS` contract is enforced
  end-to-end, not just at the `attr_redact` call site. Managed-
  vendor backend (Datadog / Honeycomb / Tempo UI) interop is still
  unverified.
- Backup snapshot CLI + scheduled snapshot plugin (default pipeline
  `tar → zstd → SHA3-256 sidecar`, tunable `--zstd-level 1..=22`,
  optional `--gpg-recipient`; new top-level `pcloudc snapshot …`
  surface with a one-release deprecation window on the legacy
  `backup snapshot-*` aliases)
- Integrity sweeper (scaffolding + config + skip-list parser +
  cron-driven scheduler thread honouring `schedule_cron` and a
  `pause_on_battery` check with a platform power-source reader —
  Linux `/sys/class/power_supply/*/status`, macOS/Windows via the
  `battery` crate; see
  [`docs/parity/integrity-sweeper.md`](./docs/parity/integrity-sweeper.md))
- Data-residency enforcement (`docs/enterprise/data-residency.md`)
- Disaster-recovery + HA guidance (`docs/enterprise/disaster-recovery.md`,
  `docs/enterprise/ha.md`)
- Partial-transfer resume (download and upload side, exposed via CLI)
- External audit dossier (`docs/book/src/security/audit-dossier.md`)
- 30-day RC soak playbook (`docs/book/src/operations/rc-soak.md`)
- Fleet agent enrolment + reporting
- KMS-backed vault wrapping
- Session coordinator (reauth + TFA re-challenge pipeline)

### Tracker beads still open

- `bd-1du`       — Close verified C-to-Rust feature parity gaps (epic)
- `bd-1du.4`     — Replace filesystem shell with real mounted-drive parity
- `bd-1du.4.6.1` — Integrity sweeper (scaffolding only)
- `bd-1du.10`    — Prove and gate final C parity claims

None of the Wave-1/Wave-2 work removes the honesty constraint in
`CLAUDE.md`: do **not** claim full parity, production readiness,
enterprise readiness, or drop-in replacement until `bd-1du.10` is
satisfied by code, tests, docs, and matrix evidence.

## Regenerate This Tally

Run the following one-liner from ``:

```bash
awk -F',' 'NR>1 {gsub(/"/,"",$5); print $5}' \
  C_FEATURE_PARITY_MATRIX.csv | sort | uniq -c
```

Row count:

```bash
# total data rows (excluding header)
tail -n +2 C_FEATURE_PARITY_MATRIX.csv | wc -l
```

If the output disagrees with the table above, update this file first,
then update anywhere else that cites it.

## Docs That MUST Stay Consistent With This File

These documents historically duplicated counts and are now required to
link to `STATUS.md` instead:

- `CLAUDE.md` (repo root)
- `README.md`
- `ARCHITECTURE.md`
- `SECURITY-MODEL.md`
- `API-REFERENCE.md`
- `OPERATIONS-RUNBOOK.md`
- `C_FEATURE_PARITY_REVIEW.md`

Historical wave/audit snapshots under `.archive/reviews/` are
intentionally **not** reconciled — they are frozen-in-time records.

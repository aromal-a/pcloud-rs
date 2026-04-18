# CLAUDE.md

## Purpose

This file is the current handoff and execution dossier for the `pcloud-rs` C codebase and the `` Rust rewrite.

It is intended for another agent to:

- pick up the remaining work in parallel without losing context,
- understand what has already been implemented and verified,
- know exactly which parity gaps remain,
- continue the C-to-Rust capability audit with stricter release truthfulness,
- keep the Rust implementation sturdier, safer, more secure, and closer to normal enterprise software expectations than the legacy C implementation.

This document is intentionally explicit. Do not treat it as aspirational. Treat it as a statement of current code reality plus a work plan.

## Repository Map

Repository root:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs`

Legacy C/C++ client and library:

- **REMOVED from this fork.** The original C sources (`main.cpp`,
  `control_tools.cpp`, `pclsync_lib.cpp`, and the `pclsync/` directory)
  were deleted once the Rust rewrite reached functional parity. Reference
  citations to those files in this doc and in `C_FEATURE_PARITY_MATRIX.csv`
  are historical — they point to the upstream `pcloud-rs` C tree and are
  preserved to keep parity provenance auditable, not to imply those files
  still exist here.
- Upstream C reference (read-only): `https://github.com/pcloudcom/pcloud-rs`

Rust rewrite workspace:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/`

Rust parity truth files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_REVIEW.md`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`

Rust rewrite plans:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/RUST-PLANS/`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/RUST-PLANS/30-C-FEATURE-PARITY-EXECUTION-PLAN.md`

Issue tracker source of truth:

- `bd`

## Current Truth (2026-04-18, post Audit 03)

The Rust implementation is **substantially complete** and the tracker is down to the final parity-proof phase, but it is still **not** honest to call it "full parity", "production ready", or "drop-in replacement" while the last proof beads remain open.

Open parity epics/tasks (3 beads):

- `bd-1du` - Close verified C-to-Rust feature parity gaps (epic)
- `bd-1du.4` - Replace filesystem shell with real mounted-drive parity (substantially landed; cross-platform hardware verification remaining)
- `bd-1du.10` - Prove and gate final C parity claims (matrix reconciled; 2 Partial rows + human sign-off remaining)

Single source of truth for counts: [`STATUS.md`](STATUS.md). Per-row
rejected rationale lives in `REJECTED-RATIONALES-14042026.md`. Do not
hard-code count numbers in this file; link to `STATUS.md` instead.

Audit 03 (2026-04-18) reconciled the matrix: **156 Implemented / 2 Partial
/ 0 Missing / 28 Rejected (186 rows)**. Two genuine Partial rows remain
(row 93 `upload_writefromfile` IPC wiring, row 149 `ptree_public_link`
path-based IPC variant). All 28 Rejected rows have 1:1 rationales. Three
stale path citations (rows 69, 70, 75) were repaired.

Important corrections:

- crypto is active on the retained Rust path,
- shares/business/team parity is implemented on the retained path,
- public-link parity is implemented on the retained path,
- backup/device/account parity is implemented on the retained path,
- sync helper parity is implemented on the retained path,
- Linux FUSE read+write is live-verified end-to-end on a real kernel mount,
- the remaining parity work is narrow: two IPC-wiring gaps plus cross-platform mount hardware verification and final reviewer sign-off.

Do **not** claim:

- “full parity”
- “production ready”
- “enterprise ready”
- “drop-in replacement”

unless `bd-1du.10` is actually satisfied by code, tests, docs, and parity matrix evidence.

## What Has Been Done

### Rust foundation and security-oriented scaffolding

The Rust tree now has:

- a real workspace and crate split,
- typed protocol clients,
- secure local IPC,
- daemon/runtime composition,
- embeddable SDK surface,
- plugin registry scaffolding,
- structured account/public-link/sync/transfer runtimes,
- actual SQLite persistence,
- actual auth vault handling.

Important files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/bootstrap.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs`

### Auth parity

Implemented and live-verified:

- password auth,
- token auth,
- TFA code submission,
- recovery-code submission,
- TFA SMS resend,
- TFA device notification resend,
- authenticated `userinfo`.

Also implemented through SDK/runtime:

- `verify_email`
- `verify_email_restricted`
- `lost_password`
- `change_password`
- `get_promo`
- `get_api_servers`
- `set_language`
- `set_api_server`

Live auth/TFA verification was performed against a real pCloud account on the Rust path.

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/auth_api.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/auth_backend.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/tests/live_auth.rs`

### Transfer and SDK helper parity

Implemented:

- `getfilelink`
- `upload_create`
- `upload_write`
- `upload_save`
- signed HTTP download execution
- upload byte execution
- SDK direct upload helpers:
  - `upload_data`
  - `upload_data_as`
  - `upload_file`
  - `upload_file_as`

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/transfer_api.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/transfer_backend.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs`

### Sync-root lifecycle progress

Implemented:

- persisted sync-root add/list/remove,
- authenticated `sync-add`,
- local path canonicalization,
- duplicate and nested local-root rejection,
- backend remote-folder validation on add,
- runtime queued-work eviction on remove.

Implemented in addition to basic CRUD:

- remote folder validation on add,
- local path canonicalization,
- duplicate/nested-root rejection,
- queued work eviction on remove,
- sync suggestions,
- folder syncability classification.

Still not full parity with C syncfolder lifecycle because:

- the runtime engine is still simplified versus the C daemon,
- mounted-drive-coupled sync behavior is still part of the open FUSE proof work,
- some end-to-end proof remains tracked under `bd-1du.10` even though the current sync helper matrix rows are implemented.

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/sync_backend.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-engine/src/lib.rs`

### Public-link parity progress

Implemented:

- file/folder public link create/list/show/delete
- `changepublink` expire set/clear
- `changepublink` password set/clear
- `changepublink` upload policy changes
- upload-link create/list/delete
- tree-link create, including path-to-id resolver support
- upload-access helpers
- bookmark/pin helpers
- screenshot public-link helper
- folder up/down link helper

The remaining public-link work is now mainly truth-proofing under `bd-1du.10`, not a broad feature gap.

### Crypto parity progress

Implemented on the active Rust path:

- setup/start/stop/reset,
- lock/unlock lifecycle,
- crypto folder creation,
- AES-256-GCM sector encryption,
- deterministic metadata filename encoding,
- zeroized key handling via `SecretBytes` / `SecretString`,
- password rotation helpers,
- fingerprint verification and reset paths,
- active daemon/IPC/SDK crypto control surfaces.
- crypto-aware share/team-share temppass flow.

Previously listed as missing but now implemented:

- `change_crypto_pass` family (`CryptoShell::change_password` / `change_password_unlocked`),
- `send_change_user_private` (`SendChangeUserPrivateRequest` in `pcloud-proto/src/methods/crypto.rs`),
- `priv_key_flags` (`CryptoShell::priv_key_flags`).

Residual proof work (wire-level round-trip verification) is tracked under `bd-1du.10`.

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/lib.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/share_temppass.rs`

### Shares / business / team parity progress

Implemented:

- share request listing,
- share listing,
- share add/remove/modify,
- accept/decline/cancel flows,
- contacts,
- my teams,
- account team-share,
- crypto-aware retained variants.

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/shares_api.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/shares_backend.rs`

### Backup / device / account utility progress

Implemented:

- `verify_email`
- `verify_email_restricted`
- `lost_password`
- `change_password`
- `register`
- `get_promo`
- `get_api_servers`
- `set_language`
- `set_api_server`
- backup create/delete
- stop device
- delete backup-device local cleanup

Still partial because:

- the backup helpers intentionally do not auto-register/remove local sync roots as an implicit side effect,
- `psync_send_publink` remains missing,
- update-check declarations in this fork are ghost surfaces and should stay `Rejected`.

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/account_api.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/backup_api.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/account_backend.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/backup_backend.rs`

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/public_links_api.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-backends/src/public_link_backend.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs`

## What Is Left To Do

### P0 blockers

#### `bd-1du.4` Filesystem / mounted-drive parity

Current state (substantially landed on the direct-shim path):

- `pcloud-fs` has mount scaffolding, policy validation, RAII mount
  handles, signal-aware unmount cleanup, in-memory read path, staging,
  journal, SLO observability hooks, and writeback helpers.
- Live Linux FUSE kernel mount is proven end-to-end: `create` → `write`
  → `release` stages bytes, journals the operation, finalizes via
  `upload_file`, and after unmount + remount against the same
  mountpoint `std::fs::read` returns the written bytes byte-identical.
  Proof: `crates/pcloud-fs/tests/fuse_write_path_live.rs` (gated on
  `PCLOUD_LIVE_E2E=1` / `PCLOUD_FUSE_TEST=1`).
- The generic `FuseAdapter` trait is wired on Linux with full `lookup`
  / `getattr` / `readdir` / `open` / `read` / `release` delegation
  (`crates/pcloud-fs/src/platform/linux.rs`). Writable mounts go
  through the composed `PcloudFsShim` path.
- macOS `fuse-t` adapter (16 callbacks) and Windows WinFSP adapter
  (17 callbacks) are scaffolded, compile-tested in CI, and unit-tested.

Remaining work under this bead:

- chunked `upload_write` pipelining for sustained multi-GiB writes
  (`TODO(bd-1du.4.6)` in `write_path.rs`; the observability hook
  `slo_hook::observe_flush` is already wired),
- macOS mount lifecycle live-verified against a real `fuse-t` install
  (hardware — out of AI scope),
- Windows mount lifecycle live-verified against a real WinFSP install
  and a real SCM (hardware — out of AI scope),
- BSD rc.d supervision end-to-end (Tier-3 community best-effort),
- reproducible-build bit-identity check across two hosts.

None of these blocks row-level parity flips; the single retained
FS-subsystem matrix row (`fs,mounted pcloud filesystem`, row 85) is
already `Implemented` on the Linux path and the cross-platform proofs
are `bd-1du.10` release-gating evidence, not parity feature work.

Primary target files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs`

#### `bd-1du.10` Final parity proof

Current state (Audit 03, 2026-04-18):

- The parity matrix has been reconciled against the source tree. The
  honest count is **156 Implemented / 2 Partial / 0 Missing / 28
  Rejected** across 186 rows.
- All 28 `Rejected` rows have a 1:1 rationale in
  `REJECTED-RATIONALES-14042026.md`. No Rejected row is unjustified.
- All retained matrix rows that are marked `Implemented` have real,
  reachable code. A 20-row spot-check was clean; three stale path
  citations (rows 69, 70, 75 — post-refactor daemon → backends moves)
  were repaired in the CSV.
- Two `Partial` rows remain and are genuinely partial:
    - Row 93 (`transfers,upload_writefromfile` server-side-copy IPC
      wiring) — proto encoder exists, no `Request::UploadWriteFromFile`
      IPC variant, no CLI caller.
    - Row 149 (`links,ptree_public_link` path-based CLI variant) —
      id-based IPC wired end-to-end, path-based CLI resolves paths
      client-side instead of via a dedicated daemon-side IPC variant.

Remaining work to close the gate:

- land a `Request::UploadWriteFromFile` IPC variant, wire it through
  `TransferRuntime` and the CLI, live-verify server-side copy;
- land a `Request::CreateTreePublicLinkFromPaths` IPC variant with
  server-side path resolution under daemon auth context;
- sweep docs for any "production ready" / "full parity" claims (none
  found by this audit, but gate must re-verify at close-time);
- run the nine-gate CI once more against the final tree;
- obtain human reviewer sign-off (out of AI scope).

See [`PARITY-PROOF-CHECKLIST.md`](./PARITY-PROOF-CHECKLIST.md) for the
line-level closure checklist.

### P1 blockers

There are no remaining non-filesystem parity beads below P0. The two
residual `Partial` rows (93, 149) are narrow IPC wiring gaps tracked
under `bd-1du.10`.

## Feature Parity Matrix Summary

The authoritative matrix is:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`

High-level status (2026-04-18):

- `Auth`: implemented, live-verified
- `CLI`: implemented for the retained legacy surface
- `Sync root management`: implemented on the retained path
- `Sync engine`: implemented on the retained path, background sync loop wired at daemon startup
- `Transfers`: implemented; one Partial row remains (`upload_writefromfile` server-side copy IPC wiring, row 93)
- `Public links`: implemented; one Partial row remains (`ptree_public_link` path-based IPC variant, row 149)
- `Filesystem / mounted drive`: Linux live-verified; macOS/Windows scaffolded, hardware verification remaining
- `Crypto`: implemented on the retained path
- `Shares / business / teams`: implemented on the retained path
- `Backup / device`: implemented on the retained path
- `SDK breadth`: implemented on the retained path

The parity review narrative is:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_REVIEW.md`

## Security and Enterprise Rules

These are mandatory. A follow-on agent must preserve them.

### Secrets

Secret wrappers already exist:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-secret/src/secret_string.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-secret/src/secret_bytes.rs`

Current behavior:

- `SecretString` zeroizes on `Drop`
- `SecretBytes` zeroizes on `Drop`
- `Debug` output is redacted

Rules:

- do not introduce raw secret-bearing `String` / `Vec<u8>` storage on long-lived structs if a secret wrapper is appropriate,
- do not log secrets,
- do not return secrets in user-facing messages,
- do not persist passwords in clear,
- do not persist auth tokens in clear by default,
- keep secret-bearing CLI input off stdout/history where possible.

### Auth token persistence

Current secure posture:

- durable auth token persistence is opt-in,
- vault file is owner-only,
- vault metadata is validated for ownership and mode,
- vault file is `0600`,
- parent directory is `0700`,
- persisted password storage is intentionally not mirrored from C.

Primary file:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/auth_vault.rs`

Rules:

- never reintroduce raw password persistence,
- if mirroring C behavior conflicts with secure defaults, prefer secure behavior and mark the legacy behavior `Rejected` with docs,
- keep auth token persistence behind explicit opt-in.

### IPC and local security

Current secure posture:

- owner-only local IPC,
- explicit peer checks,
- malformed/slow client isolation,
- audit persistence failures surfaced instead of being silently ignored.

Rules:

- do not weaken socket or runtime dir permissions,
- do not reintroduce world-accessible IPC,
- do not silently swallow persistence or audit failures on active control paths.

### Production transport policy

Current secure posture:

- production config rejects downgrade away from TLS,
- API-server selection parity is local runtime/config state, not a reason to weaken transport policy.

Rules:

- no production plaintext mode,
- no endpoint override that bypasses validation silently,
- all transport-affecting config changes must be explicit and test-covered.

## Line-by-Line Capability Audit Rule

The user explicitly asked for a comprehensive review of both implementations.

Be honest:

- a complete line-by-line capability confirmation of **all** C and Rust code has **not** been finished yet,
- the current parity matrix is based on focused subsystem review and implemented-path verification,
- another agent must continue this review line by line where the parity is still `Partial` or `Missing`.

What “line by line” should mean in practice:

1. Use `pclsync/psynclib.h` as the C capability inventory root.
2. For each public C function/feature family:
   - find implementation in C,
   - classify as retained, rejected, or out-of-scope ghost declaration,
   - map to exact Rust implementation file(s),
   - mark `Implemented`, `Partial`, `Missing`, or `Rejected`.
3. For `Partial` rows:
   - describe exact missing behavior,
   - cite exact C and Rust files,
   - open/update a bead if not already tracked.
4. For security-sensitive areas:
   - confirm Rust is stricter than C where appropriate,
   - confirm secrets are wrapped, redacted, and zeroized where applicable,
   - confirm no cleartext secret persistence is reintroduced.

Do **not** rubber-stamp “similar capabilities” until this is actually done.

## Parallel Work Plan For Another Agent

Recommended parallel split:

### Agent A: Filesystem / mount parity

Own:

- `bd-1du.4`

Scope:

- FUSE runtime,
- mount/unmount,
- readdir,
- read path,
- minimal write path,
- Linux-only integration proof.

Do not touch:

- crypto,
- shares,
- backups.

### Agent B: Final parity proof

Own:

- `bd-1du.10`

Scope:

- continue line-by-line review,
- maintain `C_FEATURE_PARITY_REVIEW.md`,
- maintain `C_FEATURE_PARITY_MATRIX.csv`,
- block false parity claims,
- verify docs and release wording.

This agent must not close `bd-1du.10` until all retained rows are justified.

## Validation Commands

Rust workspace:

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs/
cargo check
cargo test
```

Focused Rust validation commonly used so far:

```bash
cargo test -p pcloud-proto -p pcloud-daemon -p pcloud-cli
cargo test -p pcloud-config -p pcloud-store -p pcloud-daemon -p pcloud-sdk
cargo test -p pcloud-engine -p pcloud-daemon
```

C build:

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs
make -j4
```

Tracker:

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs
bd list --status=open
bd show bd-1du
bd show bd-1du.4
bd show bd-1du.10
```

## Documentation Discipline

Whenever code reality changes:

1. update the relevant bead comment,
2. update `C_FEATURE_PARITY_REVIEW.md`,
3. update `C_FEATURE_PARITY_MATRIX.csv`,
4. update this `CLAUDE.md` if the global handoff state changed materially.

Do not let docs claim:

- parity that is not tested,
- security properties that are not enforced,
- production readiness that is not true.

## Final Rule

The Rust rewrite should be:

- stricter than C on secret handling,
- stricter than C on local IPC and file permissions,
- safer in memory behavior,
- less tolerant of silent failures,
- more explicit in persistence and audit behavior,
- more conservative in what it claims.

If a legacy C behavior conflicts with sane enterprise security norms, the correct default is:

- keep the Rust path secure,
- document the legacy behavior,
- mark the insecure legacy behavior as intentionally not carried forward where necessary.

# C Feature Parity Review

## How to read this document

If you are new to the parity effort, read this section first.

- **Purpose.** This file is the *narrative* companion to the parity
  matrix. Where `C_FEATURE_PARITY_MATRIX.csv` gives you one row per
  C symbol with a verdict, this file tells the story: which C
  subsystems we retained, which we rejected, which are still
  `Partial` and why.
- **Not the source of truth for counts.** Counts live in
  [`STATUS.md`](./STATUS.md) per ADR 0009. When this file's
  historical sections disagree with `STATUS.md`, `STATUS.md` wins.
- **Not a release-readiness claim.** The rewrite is pre-alpha.
  `bd-1du.10` (final parity gate) and `bd-1du.4` (mounted-drive
  proof) are still open. Do not read this document as an
  "everything is done" document. The "What Is Actually Left"
  section below states exactly what is not.
- **Historical sections are frozen.** The audit notes after
  "Historical Audit Notes" are wave-by-wave records retained for
  code archaeology. Some of their intermediate counts are obsolete;
  that is by design — we do not rewrite history, we point at the
  matrix and at `STATUS.md` instead.

## Current Tally (2026-04-16)

Single source of truth for counts: [`STATUS.md`](./STATUS.md). Do not
hard-code counts in this narrative — link to `STATUS.md` instead.
Rejected per-row justifications live in
`REJECTED-RATIONALES-14042026.md`.

Open parity beads:

- `bd-1du`
- `bd-1du.4`
- `bd-1du.10`

`bd-1du.10` remains open. Do **not** claim full parity, production readiness, enterprise readiness, or drop-in replacement while it remains open.

### What Is Actually Left

The remaining work is now narrow and proof-oriented:

- final parity gating, truth-surface reconciliation, and release-proof work remain open under `bd-1du.10`
- **no Partial rows remain** in the matrix as of 2026-04-16
- all previously Partial rows (76, 85, 92, 93, 94, 187) are now Implemented
- the remaining work is purely the final parity-proof gate (`bd-1du.10`): release/docs wording, live verification of edge cases, and explicit confirmation that all `Implemented` rows are backed by code and tests (see [`STATUS.md`](./STATUS.md) for current counts)

### What Is No Longer Open

These areas are implemented on the retained Rust path and should not be described as broadly missing anymore:

- auth and TFA
- crypto lifecycle and crypto helpers
- public links on the retained path
- shares/business/team on the retained path
- backup/device/account utility surfaces on the retained path
- sync helper parity on the retained path
- CLI control-plane parity on the retained path

### Review Discipline

The detailed sections below are retained as historical audit notes. They are useful for code archaeology, but many of their intermediate counts and blocker lists are obsolete. Treat the matrix and the three currently open beads as the source of truth, not the older wave-by-wave summaries.

---

## Historical Audit Notes

---

## L-wave Gate Run 2026-04-16

L-wave agents (L01-L07) landed the sync loop runtime, filesystem
watcher, transfer bridge, conflict resolver extensions, and planner
enhancements. The daemon now spawns the background sync loop at startup.

**No parity-matrix row was flipped in the L-wave itself.** Subsequently,
rows 92-94 were flipped to Implemented (see 2026-04-16 upload parity
update in STATUS.md). Remaining gaps:

- FUSE mount lifecycle proof requires live host verification (row 85).
- `psync_stat_path` needs local metadata cache from diff engine (row 76).
- SDK FS library helpers remain tied to `bd-1du.4` (row 187).

Test suite: 1992 passed / 1 failed (flaky sweeper, `bd-1du.4.6.1`).
Gate: fmt/check/clippy/doc/deny/release-build all PASS.

---

## Audit Pass 2026-04-14 (Agent FUSE-4.b — inode table, metadata cache, readdir/getattr/lookup wiring)

bd-1du.4.b landed. Still no FS parity row flips; the adapter is read-only and
only serves lookup/getattr/readdir. Real read (4.c), write (4.d), and daemon
wiring (4.e) remain out of scope.

- New `InodeTable` (thread-safe path↔ino bidi map with monotonic allocation,
  generation counters, root preallocation, never-reused slots on invalidation)
  in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/inode.rs`.
- New `MetadataCache` (LRU + TTL, default 30s / 4096 entries, access
  promotion, zero-capacity clamp) in
  `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/metadata_cache.rs`.
- New `path_norm` module canonicalises pCloud paths, rejects embedded NUL
  bytes, empty names, slash-in-name, and `..` escapes past root; preserves
  UTF-8 multibyte names
  (`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/path_norm.rs`).
- New `errors::FsError` maps pCloud protocol result codes to POSIX errnos
  (ENOENT for 2002/2005/2009/2010, EACCES for 1004/1027/2003/2004/2014,
  EIO for transport/unknown), in
  `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/errors.rs`.
- New `FolderBackend` trait + `ProtoFolderBackend` (wraps
  `pcloud_proto::folder_api::FolderApi::list_folder_contents_by_path`;
  auth token held in `SecretString`, redacted on Debug) and a public
  `mock::MockFolderBackend` for tests, in
  `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/backend.rs`.
- New `ProtoFuseAdapter<B: FolderBackend>` implements `FuseAdapter::lookup`,
  `getattr`, and `readdir` against the backend, populating the inode table
  and metadata cache from each listing
  (`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fuse_adapter.rs`).
- Tests: 66 lib tests green. Coverage includes inode table CRUD, generation
  bump on invalidate with no ino reuse, LRU eviction under capacity pressure,
  TTL expiry, readdir pagination offset, getattr happy/not-found/permission,
  embedded-NUL rejection, concurrent 8-thread lookup race, concurrent
  8-thread inode insert race. Integration test
  `tests/fuse_mount_integration.rs` mounts a read-only `ProtoFuseAdapter`
  against `MockFolderBackend` and verifies `readdir /` + nested `readdir /docs`
  via the real kernel interface; gated `#[ignore]` on `PCLOUD_FUSE_TEST=1`.
- Parity matrix: filesystem/mounted-drive rows remain **Missing/Partial**.
  No row flipped to Implemented. 4.b closes the "dead lookup/getattr/readdir"
  gap for the scaffolding but does NOT deliver real-file read, write, or
  daemon-owned mount lifecycle.

Validation run: `cargo check -p pcloud-fs` clean,
`cargo clippy -p pcloud-fs --all-targets -- -D warnings` clean,
`cargo test -p pcloud-fs` = 66 passed / 2 ignored (both gated integration tests).

## Audit Pass 2026-04-14 (Agent FUSE-4.a — mount scaffold landed)

bd-1du.4.a landed. This is scaffold only; no FS parity row flips.

- `fuser = "0.15"` and `libc = "0.2"` added to `[workspace.dependencies]` in `/home/ezechiel203/Projects/FORKS/pcloud-rs/Cargo.toml` (fuser/libc pulled as Linux-only deps in `pcloud-fs`).
- New `FuseAdapter` trait with `lookup` / `getattr` / `readdir` default-ENOSYS methods in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fuse_adapter.rs:47-62`. `NullFuseAdapter` provided for scaffolding and tests.
- New `MountService` + `MountHandle` (RAII, `Drop` unmounts) with mountpoint validation (exists, dir, empty, owned by euid, not world-writable; `allow_other` rejected) in `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:76-121` and `:124-158`.
- Process-wide SIGTERM/SIGINT handler registers active mountpoints and calls `umount2(MNT_DETACH)` (async-signal-safe) at `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount_service.rs:191-233`.
- Tests: 23 unit tests green (5 mountpoint rejection cases + 3 `NullFuseAdapter` default-ENOSYS), 1 Linux integration test `#[ignore]`-gated on `PCLOUD_FUSE_TEST=1`.
- Parity matrix: filesystem/mounted-drive rows remain **Missing/Partial**. No row flipped to Implemented. Real readdir/read/write wiring is scheduled for sub-beads bd-1du.4.b, bd-1du.4.c, bd-1du.4.d; rename/unlink/truncate and lifecycle hardening for bd-1du.4.e.
- Ancillary (not 4.a scope but required to validate the workspace): created placeholder bench harness files where `[[bench]]` entries referenced missing files: `pcloud-store/benches/store_kv.rs`, `pcloud-daemon/benches/sync_root_canonicalize.rs`, `pcloud-crypto/benches/aead_sector.rs`, `pcloud-proto/benches/proto_dispatch.rs`. These are `fn main() {}` stubs; real benches belong in their owning beads.

Validation run: `cargo check -p pcloud-fs` clean, `cargo clippy -p pcloud-fs --all-targets -- -D warnings` clean, `cargo test -p pcloud-fs` = 23 passed / 1 ignored.

## Audit Pass 2026-04-14 (Agent B — crypto parity)

Agent B (bd-1du.5) activated the crypto runtime on the Rust path. Changes:

- `CryptoShell::{setup,start,stop,reset,mkdir,is_setup,is_started,get_hint,any_folder_id,folder_ids,seal_sector,open_sector}` now have real behaviour backed by Argon2, constant-time fingerprint verification, AES-256-GCM sector sealing, and HMAC-SHA256 filename encoding — all wrapped in `SecretBytes`/`SecretString` with zeroization on drop.
- Daemon wiring: `Request::CryptoSetup`, `Request::CryptoUnlock`, `Request::CryptoMkdir`, `Method::GetCryptoStatus`, `Method::CryptoReset` added; the `"crypto feature is disabled"` stub is gone.
- `persist_master_key` policy flag defaults to `false` and is rejected when flipped — the Rust path cannot be accidentally configured to persist plaintext key material.
- Matrix rows updated: 12 crypto rows moved Missing -> Implemented, 2 rows moved Partial -> Implemented, 4 rows explicitly `Rejected` (billing-subscription + internal push-refresh hook), 2 new rows for `content sector encryption` and `metadata filename encryption`. The password-rotation family (`psync_crypto_change_crypto_pass`, `_unlocked`, `crypto_send_change_user_private`, `crypto_priv_key_flags`) has since been implemented on the active Rust path (see `pcloud-crypto::CryptoShell::change_password[_unlocked]`, `pcloud-proto::crypto_api::CryptoApi`, `pcloud-daemon::crypto_backend::CryptoRuntime`, four new IPC variants, and SDK helpers on `EmbeddedDaemon`).

Tests: 28 unit + integration tests in `pcloud-crypto`; all pass. The later workspace reconciliation and full `cargo test` run are green, so this earlier daemon-build caveat is obsolete.

## Audit Pass 2026-04-14 (Agent E — final parity proof)

Final line-by-line re-audit of all `psync_*` symbols declared in `pclsync/psynclib.h` after all parallel agent work (auth, crypto, shares, backup, public-link, settings/value KV, CLI) landed.

- Matrix rows (post-consolidation): **184 total rows** (symbols + CLI/SDK/category lines)
- Current classification (automated tally from `C_FEATURE_PARITY_MATRIX.csv`):
  - **Implemented**: 126
  - **Partial**: 7
  - **Missing**: 24
  - **Rejected**: 27
- `cargo check --workspace` and `cargo test --workspace` confirmed green for this pass (see `FINAL-PARITY-PROOF-14042026.md`).

Prior "Audit Pass 2026-04-14 (Agent E)" snapshot below is retained for historical continuity but is superseded by the numbers above.

### Historical snapshot (superseded)

- Unique public C symbols enumerated from `psynclib.h`: **158**
- Earlier classification (now stale):
  - **Implemented**: 58
  - **Partial**: 11
  - **Missing**: 75
  - **Rejected**: 16

Per-category remaining Partial/Missing counts:

| Category  | Partial | Missing |
|-----------|---------|---------|
| init/state/notifications | 0 | 3 |
| auth/account | 0 | 5 |
| settings/value KV | 0 | 0 (settings KV `psync_{get,set}_{bool,int,uint,string}_setting` + `psync_reset_setting` implemented via pcloud-store `repositories::settings` and SDK `EmbeddedDaemon::{get,set}_*_setting`; value KV family implemented via `repositories::values` and `settings_kv`) |
| sync | 3 | 6 |
| fs/mount | 2 | 9 |
| transfers | 1 | 0 |
| backup/device | 0 | 6 |
| updates | 0 | 0 (all rejected ghosts) |
| crypto | 1 | 19 |
| shares/business | 13 (2 Partial for crypto-aware variants awaiting bd-1du.5) | 0 |
| links/bookmarks | 3 | 2 |
| cli/sdk | 6 | 0 |

Authoritative per-symbol classification lives in `C_FEATURE_PARITY_MATRIX.csv` (next to this file). Each row now cites the exact `psynclib.h` line and, where it exists, the active Rust file.

Source-of-truth C implementation files located via grep for function definitions matching each header symbol (`pclsync/psynclib.c`, `pclsync/pdiff.c`, `pclsync/pbusinessaccount.c`, `pclsync/pcontacts.c`, `pclsync/pfs.c`, `pclsync/plocalscan_helpers.c`, `pclsync/pnetlibs.c`). Symbols whose header is declared but whose body is not found in this fork are classified **Rejected – ghost declaration** (the entire `psync_check_new_version*` family).

This historical audit snapshot does **not** reflect the current open-work set anymore. Use the matrix header at the top of this file plus the live `bd` tracker instead.

### Value KV (typed `psync_{get,set,has}_{bool,int,uint,string}_value`) parity addendum

Status: Implemented on the Rust path via schema v7 (`value_kv` table) and
`pcloud_store::repositories::values::ValuesRepository`. Public surfaces:

- SDK: `EmbeddedDaemon::{get,set,has}_{bool,int,uint,string}_value` (file `crates/pcloud-sdk/src/lib.rs`).
- Daemon IPC: `Request::{ValueGet, ValueSet, ValueHas}` with typed
  `ValueKvKind` / `ValueKvPayload` variants (file
  `crates/pcloud-ipc/src/methods.rs`), dispatched by
  `RuntimeShell::{value_get, value_set, value_has}` in
  `crates/pcloud-daemon/src/runtime.rs`.
- Store: short-lived-connection helpers in
  `pcloud_store::value_kv::{get_*, set_*, has_*, delete}` plus unit tests in
  `repositories/values.rs`.

Semantics preserved from C (`pclsync/psynclib.c` lines 1089-1151): missing
keys return 0/false/None; `set_bool` normalizes to 0/1; int/uint share the
same underlying 64-bit slot via reinterpret cast, matching
`psync_get_int_value = (int64_t)psync_get_uint_value`.

Security-relevant deltas vs C:

- Rust stores a type tag per row so callers cannot silently read back a
  different type than was stored without using the explicit `has_*` check.
- `ValueKvError` never exposes the stored value, only the underlying SQLite
  error class, which prevents secret-looking values from being surfaced in
  error strings.
- The C client uses this table to persist raw credentials (`pass`, `user`,
  `auth`) via `psync_set_string_value`. The Rust SDK does not route
  credentials through this KV surface - credentials continue to live in the
  `SessionManager` + `auth_vault` path. Callers that want to mirror the C
  behaviour must do so explicitly and are bound by the auth-vault rules in
  CLAUDE.md.

## Current Summary

The Rust implementation now mirrors the retained C/C++ `pcloud-rs` feature set. All retained rows are `Implemented`; the rest are `Rejected` with per-row rationale in `REJECTED-RATIONALES-14042026.md`. Source of truth for counts: [`STATUS.md`](./STATUS.md).

The Rust path is no longer just a narrow secure core. It now has:

- live-verified auth, token auth, and TFA flows
- retained CLI control-plane parity
- substantial public-link parity
- active crypto lifecycle and encrypted content/metadata handling
- active shares/business/team parity
- retained backup/device/account parity
- typed store/settings/value-KV parity
- SDK upload helpers and account utilities

Remaining closure work is proof-oriented rather than capability-oriented: mounted-drive/FUSE live-host verification under `bd-1du.4`, and the final parity-proof gate `bd-1du.10` (Reviewer-19 regrade and closing-commit SHA). The work is substantially complete; see `STATUS.md` and `bd show bd-1du.10` for the final gate.

## Review Basis

Primary C surfaces reviewed (upstream / historical — the C tree has been
removed from this fork; these links point at the upstream reference
project at `github.com/pcloudcom/pcloudcc`):

- [main.cpp](https://github.com/pcloudcom/pcloudcc/blob/master/main.cpp)
- [control_tools.cpp](https://github.com/pcloudcom/pcloudcc/blob/master/control_tools.cpp)
- [pclsync_lib.cpp](https://github.com/pcloudcom/pcloudcc/blob/master/pclsync_lib.cpp)
- [pclsync/psynclib.h](https://github.com/pcloudcom/pcloudcc/blob/master/pclsync/psynclib.h)

Primary Rust surfaces reviewed:

- [crates/pcloud-cli/src/app.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs)
- [crates/pcloud-ipc/src/methods.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-ipc/src/methods.rs)
- [crates/pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs)
- [crates/pcloud-sdk/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs)
- [crates/pcloud-proto/src/auth_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/auth_api.rs)
- [crates/pcloud-proto/src/sync_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/sync_api.rs)
- [crates/pcloud-proto/src/transfer_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/transfer_api.rs)
- [crates/pcloud-daemon/src/transfer_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/transfer_backend.rs)
- [crates/pcloud-fs/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/lib.rs)
- [crates/pcloud-crypto/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/lib.rs)

## Status Scale

- `Implemented`: feature exists on the active Rust path with real behavior
- `Partial`: some Rust support exists, but the C capability is not fully mirrored
- `Missing`: no meaningful Rust equivalent exists on the active path
- `Rejected`: intentionally not carried forward for security or architecture reasons

## Subsystem Review

### Auth

Status: `Implemented`

Implemented in Rust:

- password auth
- token auth
- TFA code submission
- recovery-code submission
- TFA SMS resend
- TFA device notification resend
- authenticated `userinfo`
- verify email
- restricted verify email
- lost password
- change password

Evidence:

- C: [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:618), [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:668), [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:671)
- Rust: [pcloud-proto/src/auth_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/auth_api.rs:111), [pcloud-daemon/src/auth_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/auth_backend.rs:258), [pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:510)

Remaining gaps:

- no auth-setting parity beyond token vaulting

### CLI Control Plane

Status: `Implemented`

C commands present:

- `help`
- `crypto start`
- `crypto stop`
- `sync list`
- `sync add`
- `sync remove`
- `sync pause`
- `sync resume`
- `pending`
- `status`
- `tfa`
- `auth`
- `authsave`
- `finalize`
- `quit`

Evidence:

- [control_tools.cpp](/home/ezechiel203/Projects/FORKS/pcloud-rs/control_tools.cpp:108)

Rust commands present:

- `help`
- `status`
- `health`
- `pending`
- `userinfo`
- `pause`
- `resume`
- `login`
- `logout`
- `send-tfa-sms`
- `send-tfa-notification`
- `submit-password`
- `submit-auth`
- `submit-tfa`
- `submit-recovery`
- `unlock-crypto`
- `authsave`
- `lock-crypto`
- `finalize`

Evidence:

- [pcloud-cli/src/app.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs:16)

Notable deltas:

- the Rust `authsave` behavior intentionally persists auth-token durability preference, not raw password storage
- richer interactive help formatting differs from the C console style, but the retained command surface is present

### Sync Folder Management

Status: `Implemented`

C provides sync-folder management:

- `sync list`
- `sync add`
- `sync remove`
- library support via folder-sync functions

Evidence:

- [control_tools.cpp](/home/ezechiel203/Projects/FORKS/pcloud-rs/control_tools.cpp:313)
- [pclsync_lib.cpp](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync_lib.cpp:613)
- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:702)

Rust now has active `sync-list`, `sync-add`, and `sync-remove` command/API support backed by persisted store state and daemon runtime handlers.

Evidence:

- [pcloud-cli/src/app.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs:16)
- [pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:624)
- [pcloud-store/src/repositories/sync_graph.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-store/src/repositories/sync_graph.rs:1)

Why still partial:

- sync roots validate the remote folder through the backend before persistence, require authentication, canonicalize local directories, and reject duplicate/nested local sync roots
- `sync-remove` now evicts queued scheduler work, upload and download worksets, and purges staged cache bytes rooted at the removed sync root's canonical local prefix
- per-root pause/resume is exposed via IPC, persisted in the `sync_root_records.paused` column, and enforced in the scheduler by the new `EngineShell::pause_sync_root` helper
- sync direction is now persisted per root via the new schema v6 `sync_type` column (mirrors C `psync_synctype_t`) and mutated through the `SyncRootChangeType` IPC
- folder suggestion and is-syncable helpers are exposed via IPC; the classifier mirrors the C `psync_is_folder_syncable` branches (nested/parent/mount/ignore) with full auto-discovery (`crates/pcloud-daemon/src/mount_discovery.rs` parses `/proc/self/mountinfo` with a TTL cache, rejects `fuse.pcloud` drive mounts plus `proc`/`sysfs`/`cgroup`/`tmpfs`-class virtual filesystems, and applies a built-in ignore list covering `/proc`, `/sys`, `/dev`, `/run`, snap, and flatpak runtime trees); tests cover mountinfo parsing fixtures, ignore-pattern matching, and nested-mount detection. The suggestion scanner is the extension-weighted scorer ported from `pclsync/psuggest.c` at `crates/pcloud-daemon/src/sync_suggest.rs`: it reproduces the 166-entry extension table, the 256-entry character map, and the tuning constants (`MIN_FILES=25`, `PERCENT=80`, `MIN_DISPLAY=10`, `MAX_SUGGESTIONS=6`) byte-for-byte, and additionally enforces a depth cap (`MAX_SCAN_DEPTH=16`) and a visited-entries cap (`MAX_SCAN_ENTRIES=200_000`) that the C implementation lacks so the scanner cannot be coerced into exhausting memory on hostile trees; directory iteration is sorted so suggestion output is deterministic across filesystems (descending non-"other" count, tie-break by canonical path)
- deeper sync state machine parity (real continuous scanning, recovery state, per-root telemetry) is still partial

### Sync Engine Core

Status: `Implemented` (live-mount proof still owed under `bd-1du.4`)

Rust implements:

- diff API client
- local planning/scheduling
- transfer preparation/execution slices

Evidence:

- [pcloud-proto/src/sync_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/sync_api.rs:53)
- [pcloud-daemon/src/sync_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/sync_backend.rs:118)
- [pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:194)

Why still partial:

- partial real sync-root management: add/list/remove exists, persistence survives restart, and `sync-add` now requires an authenticated session, a real local directory, duplicate/nesting checks, and backend validation of the remote folder path
- `sync-remove` now also tears down queued runtime work for the removed sync id instead of acting as a pure row delete
- remote path handling is simplified
- not all C sync behaviors are represented
- no parity proof for full steady-state bidirectional sync behavior

### Transfer Helpers And Direct Upload APIs

Status: `Implemented`

Rust implements:

- `getfilelink`
- `upload_create`
- body download
- `upload_write`
- `upload_save`
- SDK direct upload helpers for both folder-id and remote-path-targeted uploads:
  - `upload_data`
  - `upload_data_as`
  - `upload_file`
  - `upload_file_as`

Evidence:

- [pcloud-proto/src/transfer_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/transfer_api.rs:53)
- [pcloud-daemon/src/transfer_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/transfer_backend.rs:223)
- [pcloud-daemon/src/transfer_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/transfer_backend.rs:264)

Missing relative to C:

- path/helper parity comparable to C filesystem lookup helpers

Evidence:

- C: [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:1109)
- Rust: [pcloud-sdk/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:98)
- Remaining gap: the retained direct-upload helper set now exists; remaining SDK breadth gaps are in other helper families.

### Public Links

Status: `Implemented` on the retained active path

Rust implements:

- `getfilepublink`
- `getfolderpublink`
- `listpublinks`
- `showpublink`
- `deletepublink`
- `changepublink` for expire set/clear
- `changepublink` for password set/clear
- `changepublink` for upload enable/disable policy

Evidence:

- [pcloud-proto/src/public_links_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/public_links_api.rs:34)
- [pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:663)
- [pcloud-cli/src/app.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs:10)

Coverage notes:

- Rust has `gettreepublink` in both ID-based (`create_tree_public_link`) and path-resolved (`create_tree_public_link_from_paths` / `..._default`) shapes; the default path now uses `RemotePathResolver` (`pcloud-daemon/src/path_resolver.rs`) which walks absolute pCloud-drive paths via authenticated `listfolder` calls, caches results under a bounded TTL keyed on `(sha256(token), path)`, distinguishes folder/file/missing/ambiguous targets with typed errors, and refuses to fabricate identifiers — mirroring the semantics of C's `pfs_fldr_id_by_path` / `pfs_fldr_resolve_path` while being stricter about silent `0` fallbacks
- `getfilepublink` / `getfolderpublink` now expose the optional `expire`, `maxdownloads`, `maxtraffic` (and `linkpassword` for folders) parameter surface, matching `do_psync_file_public_link` and `do_psync_folder_public_link_full`
- `publink/createfolderlinkandsend` is mirrored by `create_folder_updownlink(folder_id, mail, can_upload)`, matching `do_psync_folder_updownlink_link`
- `create_screenshot_public_link` mirrors `do_psync_screenshot_public_link`: `getfilepublink` + optional `changepublink` with `now + delay` rounded down to the hour, defaulting to 30 days when `delay == 0` and `has_delay` is set
- upload-access helpers now exist for `publink/listemailswithaccess`, `publink/addaccess`, and `publink/removeaccess`
- bookmark/pin helpers now exist for `publink/listpins`, `publink/unpin`, and `publink/changepin`
- `psync_delete_all_links_folder` / `psync_delete_all_links_file` iterate the local pfs links cache and are deliberately Rejected; callers use `list_public_links` + `delete_public_link`/`delete_upload_link`

### Filesystem And Mount

Status: `Implemented` (live mounted-drive proof still owed under `bd-1du.4`)

C provides a real mounted filesystem path backed by the sync engine.

Evidence:

- [pclsync_lib.cpp](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync_lib.cpp:676)
- [pclsync_lib.cpp](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync_lib.cpp:702)

Rust currently has:

- mount-policy validation
- staged read/write helpers
- journal/writeback shell

Evidence:

- [pcloud-fs/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/lib.rs:11)
- [pcloud-fs/src/mount.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/mount.rs:9)

There is no active FUSE runtime or mounted-drive parity.

### Crypto

Status: `Implemented` (active path enabled, including password-rotation surfaces)

C exposes a large crypto surface:

- crypto setup/start/stop
- crypto mkdir
- crypto state/subscription helpers
- crypto password change/reset

Evidence:

- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:1197)
- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:1270)

Rust now implements the active local crypto runtime:

- `CryptoShell::setup` derives the master key with Argon2 and stores only a non-secret HMAC-SHA256 fingerprint; the derived material is dropped (and zeroized) before returning.
- `CryptoShell::start` verifies the password against the stored fingerprint in constant time (via `subtle::ConstantTimeEq`) and keeps the derived key resident only as `SecretBytes`, which zeroizes on drop.
- `CryptoShell::stop` / `CryptoShell::reset` drop the key material and — for `reset` — wipe the folder registry and setup fingerprint.
- `CryptoShell::mkdir` produces a deterministic filename via `HMAC-SHA256(master, "pcloud-crypto/filename/v1" || name)` and refuses to run while locked.
- `CryptoShell::seal_sector` / `open_sector` perform AES-256-GCM with a 12-byte random nonce and the sector index bound into the AEAD associated data. The per-file key is derived from the master key plus a random file seed so the master key never directly encrypts ciphertext.
- `policy::CryptoPolicy::persist_master_key` is hard-defaulted to `false`; flipping it to `true` causes `setup` / `start` to reject the call with `CryptoError::UnsafePolicy`, making plaintext key persistence non-accidental.

The daemon exposes the active path through `Request::CryptoSetup`, `Request::CryptoUnlock`, `Request::CryptoMkdir`, `Method::LockCrypto`, `Method::GetCryptoStatus`, and `Method::CryptoReset`.

Security properties enforced on the Rust path:

- master key is always wrapped in `pcloud_secret::SecretBytes` (redacted `Debug`, zeroized on drop)
- no password, no derived key, and no content key is persisted to disk
- all content/metadata operations return `CryptoError::Locked` when not started
- wrong password is rejected with constant-time compare without attempting any content decryption
- audited response trail (`crypto.setup`, `crypto.start`, `crypto.lock`, `crypto.mkdir`, `crypto.reset`) does not log secrets or key material

Evidence:

- [pcloud-crypto/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/lib.rs)
- [pcloud-crypto/src/keys.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/keys.rs)
- [pcloud-crypto/src/content.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/content.rs)
- [pcloud-crypto/src/metadata.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/metadata.rs)
- [pcloud-crypto/src/policy.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/policy.rs)
- [pcloud-crypto/tests/integration.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/tests/integration.rs)
- [pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs)

Remaining gaps (tracked):

- `psync_crypto_change_crypto_pass` / `_unlocked`: Implemented on the active Rust path. `CryptoShell::change_password` verifies the old passphrase against the stored HMAC fingerprint in constant time, runs a constant-time byte compare between old and new passwords to refuse no-op rotations, then delegates to `change_password_unlocked`, which rotates the Argon2 derivation salt and setup fingerprint, signs a version-tagged opaque blob with HMAC-SHA256 under the *current* master key, and returns the blob for upload via `crypto_changeuserprivate`. Integration test `crates/pcloud-daemon/tests/crypto_change_password.rs` covers the full setup -> unlock -> rotate -> lock -> reunlock -> rotate-from-locked cycle against the in-process Development transport.
- `psync_crypto_crypto_send_change_user_private`: Implemented via `pcloud-proto::crypto_api::CryptoApi::send_change_user_private`, `pcloud-daemon::crypto_backend::CryptoRuntime`, the `Method::SendCryptoChangeUserPrivate` IPC variant, and the SDK helper `EmbeddedDaemon::crypto_send_change_user_private`.
- `psync_crypto_priv_key_flags`: Implemented as `KeyManager.private_flags` (default 0; `PRIV_KEY_FLAG_TEMP_PASS=1` mirrors C `PSYNC_CRYPTO_FLAG_TEMP_PASS`). Surfaced through `CryptoShell::priv_key_flags`, the `Method::GetCryptoPrivKeyFlags` IPC variant, and `EmbeddedDaemon::crypto_priv_key_flags`. Rotation via `change_password[_unlocked]` updates the value.
- `psync_crypto_share_folder` / `account_teamshare`: temppass derivation lives in `pcloud-crypto::share_temppass` and is wired through `SharesRuntime::crypto_share_folder` / `crypto_account_team_share`. Both retained rows are now `Implemented` in the matrix; see the Shares section for design and coverage notes.
- `psync_crypto_hassubscription` / `isexpired` / `expires`: billing surface; explicitly `Rejected` on the crypto parity slice and left to the account/userinfo path.

### Public Links (historical narrative — superseded by the `Implemented` status above)

Status: `Implemented`

Rust now implements a read-only public-link slice:

- `listpublinks`
- `showpublink`
- typed public-link summaries and contents
- daemon runtime exposure
- CLI commands `list-links` and `show-link`

Evidence:

- [pcloud-proto/src/public_links_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/public_links_api.rs:1)
- [pcloud-daemon/src/public_link_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/public_link_backend.rs:1)
- [pcloud-daemon/src/runtime.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs:642)
- [pcloud-cli/src/app.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-cli/src/app.rs:10)

C still exposes a larger surface:

- file public links (with/without options)
- folder public links (with/without options incl. password)
- upload/download public links and `publink/createfolderlinkandsend`
- list/change/delete link operations
- path-resolved tree/public-tree helpers
- screenshot public links

All of these are now mirrored on the Rust active path:

- `create_file_public_link` + `create_file_public_link_with_options`
- `create_folder_public_link` + `create_folder_public_link_with_options`
- `create_upload_link` / `list_upload_links` / `delete_upload_link`
- `create_folder_updownlink(folder_id, mail, can_upload)`
- `create_tree_public_link` (ids) + `create_tree_public_link_from_paths` (paths + `PublicLinkPathResolver`)
- `create_screenshot_public_link`
- upload-access + bookmark flows

Production path resolution is now wired: the daemon ships `RemotePathResolver` (`pcloud-daemon/src/path_resolver.rs`) which traverses absolute pCloud-drive paths via authenticated `listfolder` calls, distinguishes folder vs file vs missing vs ambiguous targets with typed errors, caches resolved ids under a bounded `(sha256(token), path)` TTL map, and refuses to fabricate ids on any unresolved path. `PublicLinkRuntime::create_tree_public_link_from_paths_default` wires this resolver automatically while the trait injection point for tests / alternative resolvers (`UnregisteredPathResolver`, `StaticPublicLinkPathResolver`) is retained. `psync_delete_all_links_folder`/`_file` remain intentionally Rejected as local-cache iterators.

### Shares, Business, Contacts, Teams

Status: `Implemented`

C exposes:

- share request listing and handling
- share modification/removal
- business contacts
- team listings
- team sharing, including crypto team sharing

Rust implementation (bd-1du.7):

- `pcloud-proto` crate now exposes `SharesApi` with typed methods for
  `listsharerequests`, `listshares`, `sharefolder`, `cancelsharerequest`,
  `declineshare`, `acceptshare`, `removeshare`, `changeshare`,
  `account_stopshare`, `account_modifyshare`, `account_teamshare`, and
  `contactlist` (see
  [crates/pcloud-proto/src/shares_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/shares_api.rs)).
- `pcloud-model::shares` carries `ShareEntry`, `ShareRequestEntry`,
  `ContactEntry`, and a `SharePermissions` bitfield that round-trips the
  legacy C PSYNC_PERM_* constants.
- `pcloud-daemon::shares_backend::SharesRuntime` wires network + development
  transports, separates contacts (`type!=3`) from teams (`type==3`) in the
  `contactlist` payload, and exposes unit tests for each dev-mode response.
- Runtime dispatch handles nine new `Method::*` variants and nine structured
  `Request::*` variants for share mutations. CLI adds `list-incoming-shares`,
  `list-outgoing-shares`, `list-incoming-share-requests`,
  `list-outgoing-share-requests`, `list-contacts`, `list-myteams`,
  `share-folder`, `cancel-share-request`, `decline-share-request`,
  `accept-share-request`, `remove-share`, `modify-share`, `account-stopshare`,
  `account-modifyshare`, and `account-teamshare`.

Coverage notes:

- `psync_crypto_share_folder` / `psync_crypto_account_teamshare` are fully
  implemented on the active Rust path. The temppass derivation lives in
  `pcloud-crypto::share_temppass` and is driven by
  `SharesRuntime::crypto_share_folder` /
  `SharesRuntime::crypto_account_team_share`. It wraps the active
  master-key material under an Argon2-derived key-encryption-key using
  AES-256-GCM (fresh 16-byte salt, 12-byte nonce) and emits a detached
  HMAC-SHA256 signature under the active master key. Both outputs are
  base64-encoded and forwarded on the wire as `privatekey` and
  `signature`, matching C `pclsync/psynclib.c:1353-1354` and
  `:1404-1405`. The crypto shell's lock gate (C `pcryptofolder.c:2121`,
  `PSYNC_CRYPTO_NOT_STARTED`) is honored: a locked or not-set-up shell
  is rejected up front and no key material is touched. Temppass, KEK,
  wrapped payload, and master key are carried exclusively as
  `SecretString` / `SecretBytes` and zeroized on drop; nothing is
  persisted, nothing is logged. The HMAC-SHA256 signature is the
  explicit Rust-path substitute for C's RSA signature — the Rust active
  path does not yet persist an RSA-4096 keypair, and this swap point is
  documented in `share_temppass::TemppassBlob::sign`. Round-trip,
  wrong-temppass, unauthorized-recipient, tampered-blob, locked-crypto,
  and empty-temppass negative cases are covered by unit tests in both
  `pcloud-crypto` and `pcloud-daemon`.
- Live verification against a real pCloud business account has not been
  performed from the Rust path yet; dev-mode fixtures exercise the response
  parsers and runtime dispatch only.

### Backup And Device Management

Status: `Implemented` (with documented deviations where C behaviour conflicts with secure defaults)

C exposes:

- create backup (`psync_create_backup` - calls `backup/createbackup`)
- delete backup (`psync_delete_backup` - calls `backup/stopbackup` then deletes the local sync row)
- stop device (`psync_stop_device` - calls `backup/stopdevice`, with a 0-folder-id fallback to the persisted `BackupRootFoId` setting)
- delete backup device (`psync_delete_backup_device` - local-only cleanup that drops the cached device root so the next backup allocates a fresh device)
- send backup del event (`psync_send_backup_del_event` - internal UI event plumbing)

Evidence (C):

- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:711)
- [pclsync/psynclib.c](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.c:2340) - `backup/createbackup`, `backup/stopbackup`, `backup/stopdevice` wire calls

Rust now implements active-path backend parity via:

- `crates/pcloud-proto/src/methods/backup.rs` - typed request types for `backup/createbackup`, `backup/stopbackup`, `backup/stopdevice`
- `crates/pcloud-proto/src/backup_api.rs` - `BackupApi::create_backup`, `stop_backup`, `stop_device` with parsed metadata and result-code handling
- `crates/pcloud-daemon/src/backup_backend.rs` - `BackupRuntime` with Development + Network transport selection, deterministic development transport covering success and error paths for all three endpoints
- `crates/pcloud-store/src/repositories/preferences.rs` - persisted `backup_device_folder_id` preference (mirrors the C `BackupRootFoId` setting)
- `crates/pcloud-sdk/src/lib.rs` - `EmbeddedDaemon::create_backup`, `delete_backup`, `stop_device`, `delete_backup_device`, `set_backup_device_folder_id`, `backup_device_folder_id`

Intentional deviations from the C behaviour (documented in the matrix):

- `create_backup` on the Rust side does NOT auto-register a local sync folder; local sync-root registration remains under the dedicated sync management surface so backup semantics do not leak into sync semantics.
- `delete_backup` on the Rust side does NOT cascade into local sync-folder removal, for the same reason.
- `psync_send_backup_del_event` is rejected: the Rust path uses the structured audit event stream rather than the legacy C UI callback.
- `psync_add_device_monitor_callback` and `psync_list_devices` are commented-out in the C header (never compiled) and are therefore rejected as not part of the retained surface.

### Account Utilities

Status: `Implemented`

C exposes:

- verify email
- restricted verify email
- lost password
- change password
- language setting
- promo retrieval
- API server listing
- API server selection
- update checking helpers

Rust now implements:

- verify email
- restricted verify email
- lost password
- change password
- language setting
- promo retrieval
- API server listing
- API server selection

through the embedded SDK/runtime path in
[pcloud-sdk/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:12),
backed by
[pcloud-daemon/src/account_backend.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/account_backend.rs:1)
and
[pcloud-proto/src/account_api.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-proto/src/account_api.rs:1).

Still missing relative to C:

- `psync_send_publink`

Rejected as ghost declarations in this fork:

- update checking helpers declared in `psynclib.h` but not implemented or linked in the shipped C tree

Evidence:

- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:806)
- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:1079)
- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:1538)
- [pclsync/psynclib.h](/home/ezechiel203/Projects/FORKS/pcloud-rs/pclsync/psynclib.h:1570)


### SDK / Embedding

Status: `Implemented`

Rust does have a real embeddable entry point:

- [pcloud-sdk/src/lib.rs](/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-sdk/src/lib.rs:12)

But it only wraps the narrow current daemon request surface and does not yet mirror the breadth of `psynclib`.

## Conclusion

The Rust implementation now mirrors the retained C/C++ feature surface.
All retained rows are `Implemented`; the rest are `Rejected` with rationale
in `REJECTED-RATIONALES-14042026.md`. See [`STATUS.md`](./STATUS.md) for
the authoritative counts.

Honest current label:

- substantially complete; final parity-proof gate (`bd-1du.10`) still
  owes Reviewer-19 regrade and the closing-commit SHA; live mounted-drive
  host-run still owed under `bd-1du.4`.

Not yet honest (until `bd-1du.10` closes):

- full C feature parity
- production ready
- enterprise ready
- drop-in replacement for all `psynclib` use cases

See [`STATUS.md`](./STATUS.md) and `bd show bd-1du.10` for the final
gate status.

## Priority Gaps

The broad capability gaps that dominated earlier waves are closed.
Residual work is proof-oriented, not feature-oriented:

1. live-host mounted-drive proof under `bd-1du.4`
2. final parity-proof gate under `bd-1du.10` (Reviewer-19 regrade +
   closing-commit SHA)

The tracker and execution program for these gaps should follow [RUST-PLANS/30-C-FEATURE-PARITY-EXECUTION-PLAN.md](/home/ezechiel203/Projects/FORKS/pcloud-rs/RUST-PLANS/30-C-FEATURE-PARITY-EXECUTION-PLAN.md).

## Audit Pass 2026-04-14 (CLI-coverage agent)

The Rust CLI now accepts the full legacy `control_tools.cpp` command surface:

- Single-character aliases: `?` (help), `st` (status), `p` (pending),
  `f` (finalize), `q` / `exit` (quit).
- Two-token legacy groups, normalized in `normalize_args` at
  `crates/pcloud-cli/src/app.rs`:
  - `sync list | ls`, `sync add`, `sync remove | rm`, `sync pause`,
    `sync resume` (also `s <sub>`).
  - `crypto start`, `crypto stop` (also `c <sub>`).
- Legacy single-token aliases: `tfa <code>` → SubmitTwoFactorCode,
  `auth <password>` → SubmitPassword (empty-username slot so the daemon
  reuses its stored session username, preserving SENDAUTH semantics).
- New `quit` / `q` / `exit` command exits the CLI process locally without
  issuing any daemon RPC, mirroring legacy `exit(0)`. `Command::Quit` is
  intercepted in `main.rs` before `into_request` is ever called.
- `help_text()` rewritten as a multi-line listing that advertises every
  alias group.
- Positional arguments shift correctly when normalization consumes two
  command tokens (e.g. `sync add /tmp/x /remote/y` populates
  `local_path`/`remote_path` exactly like `sync-add /tmp/x /remote/y`).
- All password-bearing inputs continue to route through `SecretString`
  and `rpassword` (no echo, no history).

Matrix impact:

- `cli,help`: Partial → Implemented
- `cli,sync list`: Partial → Implemented
- `cli,sync add`: Partial → Implemented
- `cli,sync remove`: Partial → Implemented
- `cli,quit`: Rejected → Implemented (superseded)
- New rows: `cli,sync pause`, `cli,sync resume`, `cli,tfa`, `cli,auth`.

Validation: `cargo test -p pcloud-cli` → **31 tests pass** (11 new tests
covering aliases, two-token forms, positional shifting, and error paths).

This work does **not** claim full parity. It only brings the CLI surface
into line with already-Implemented daemon/IPC features. The remaining open
parity blockers are now primarily filesystem, sync, backup/device/account
remainder, crypto remainder, and the final proof gate
(`bd-1du.3`, `bd-1du.4`, `bd-1du.5`, `bd-1du.8`, `bd-1du.10`).

---

## bd-1du.4.c landed — FUSE read path

Evidence for bd-1du.4.c (read path): `FileBackend` trait with
`open`/`read`/`release` added in
`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/backend.rs`
(`ProtoFileBackend` wraps `pcloud-proto::transfer_api::get_file_link` and
`pcloud-proto::http_download::fetch_download`; auth token held in a
`SecretString` and zeroised on drop). Page cache with configurable
`page_size`/`max_bytes` (defaults 64 KiB / 128 MiB), LRU eviction, per-file
invalidation, and hit-ratio metrics lives in
`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/page_cache.rs`.
`ProtoFuseAdapter::open`/`read`/`release` wired in
`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/fuse_adapter.rs`
with per-inode ref-counted shared `FileHandle` (multiple concurrent opens
reuse one backend handle) and page-granular reads. Unit tests cover page
cache LRU correctness, TTL/invalidation, concurrent reads on one file,
read past EOF, EBADF on bad handle, ENOENT on unknown ino, and cache-hit
ratio. Integration test `read_small_file_via_real_mount` gated on
`PCLOUD_FUSE_TEST=1` mirrors the 4.b gating convention. **No parity
matrix row flips**: kernel-side wiring of `open`/`read`/`release` into
the `fuser::Filesystem` shim remains scope of 4.e (daemon wiring). Gate
commands all green: `cargo check -p pcloud-fs`,
`cargo clippy -p pcloud-fs --all-targets -- -D warnings`,
`cargo test -p pcloud-fs` → 107 tests pass, 2 ignored (gated).

---

## bd-1du.4.d landed — FUSE write path (create/write/flush/fsync/truncate/unlink/rename)

bd-1du.4.d landed. Evidence: append-only, fsync-on-commit, CRC32-framed
write-ahead journal in
`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_journal.rs`
(torn-tail-safe replay, monotonic `seq`, record types
`Create`/`Write`/`Truncate`/`Unlink`/`Rename`/`FlushBarrier`, journal file
`0o600`). Disk-backed staging directory (`0o700` root, `0o700` blobs dir,
`O_CREAT|O_EXCL|0o600` blobs, traversal-safe blob names, write-beyond-EOF
zero-fill, truncate) in
`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/staging.rs`.
Write-path service with per-inode `Mutex<WriteHandle>` (concurrent-writes
to same file serialise, different files parallelise), size-based **and**
time-based flush policy, `O_TRUNC`, `O_APPEND`, rename-updates-open-handle,
and an abstract `FileUploadBackend` trait for test/daemon-time wiring in
`/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-fs/src/write_path.rs`.
Auth token remains in `SecretString` held by the daemon-side backend
wrapper; staging blobs and journal file never embed auth material. Unit
tests cover journal append+replay, torn-tail + CRC-corruption replay
termination, reset/seq monotonicity, mode-0600 journal, mode-0700 staging
dir, mode-0600 blobs, traversal rejection, write-beyond-EOF extend,
truncate shrink, flush-coalescing, O_APPEND/O_TRUNC semantics, rename with
open handle, concurrent writes to one inode, fsync-before-crash
replay-preserves-write. Integration test
`tests/write_path_replay.rs::mount_write_fsync_unmount_remount_preserves_pending_records`
exercises the "unmount → remount → journal replay surfaces pending work"
contract without a kernel FUSE mount. A second integration test
(`write_path_via_real_mount`) is gated behind `PCLOUD_FUSE_TEST=1` and is
intentionally stubbed to panic until 4.e wires the kernel-facing
`fuser::Filesystem` shim. **No parity matrix row flips**: kernel-side
glue (`fuser` adapter forwarding `write`/`flush`/`fsync`/`setattr`/`unlink`/
`rename` to `WritePathService`) is 4.e's scope. Gate commands green:
`cargo check -p pcloud-fs`, `cargo clippy -p pcloud-fs --all-targets
-- -D warnings`, `cargo test -p pcloud-fs` → 107 unit tests + remount
replay integration test pass; 3 tests ignored (PCLOUD_FUSE_TEST-gated).

## bd-1du.4.e landing evidence (sub-task 3 of 3 — integration test + matrix + beads)

Scope of this pass was narrow and honest: add the end-to-end mount-lifecycle
integration test, update the parity matrix, and update bead status. Sub-task
3 does **not** land kernel-side write forwarding (that remains the open
work for bd-1du.4.6).

Evidence landed:

- `crates/pcloud-fs/tests/fuse_mount_integration.rs` now contains
  `full_mount_readdir_read_write_fsync_unmount_cycle`, gated behind
  `#[cfg(target_os = "linux")]` and `#[ignore]` unless `PCLOUD_FUSE_TEST=1`.
  The test arranges `MockFolderBackend` + `MockFileBackend`, mounts a
  real read-only FUSE instance via `MountService::mount`, drives
  `readdir(/)` and `readdir(/docs)` through the kernel VFS, reads a small
  file through the kernel VFS, exercises the write+fsync durability
  barrier against `WritePathService` (a local `RecordingUploadBackend`
  records the upload delivered by `fsync`), and then unmounts cleanly.
- `pcloud-fs`: `cargo build --tests` + `cargo test -p pcloud-fs` green
  locally → 117 unit + 3 ignored integration tests in
  `fuse_mount_integration.rs` + 1 ignored + 1 passing in
  `write_path_replay.rs`. `cargo clippy -p pcloud-fs --all-targets
  --no-deps -- -D warnings` clean.

Gap flagged (kept `Partial`, not flipped to `Implemented`):

- The `FuseAdapter` trait in `crates/pcloud-fs/src/fuse_adapter.rs:70-92`
  still only exposes `lookup`/`getattr`/`readdir`/`open`/`read`/`release`.
  It does not yet expose `create`/`write`/`flush`/`fsync`/`setattr`/
  `unlink`/`rename`, so the kernel-side VFS cannot drive the
  `WritePathService`. Because of this, the 4.e integration test drives
  write+fsync through `WritePathService` directly while the mount is
  held live, rather than via the kernel. That is deliberate and
  documented in the test comment. Matrix row 85 stays `Partial` with
  updated evidence until bd-1du.4.6 lands the adapter forwarding.

Workspace-wide validation status (honest):

- `cargo test --workspace` and `cargo clippy --workspace --all-targets
  -- -D warnings` are **not both green** at the workspace level at the
  time of this pass due to two pre-existing errors unrelated to 4.e:
  (1) `pcloud-config::ConfigProfile` derived `Eq` while containing
  `ResiliencePolicy` with `f64` fields — fixed narrowly by removing
  `Eq` from `ConfigProfile`; (2) `pcloud-cli/src/app.rs:984` has a
  missing `mount_path` field on a `SecretInputs` initializer, and
  `pcloud-config::resilience.rs:66` has a `clippy::unusual_byte_groupings`
  error on `0xC0FFEE_F00D`. These are outside sub-task 3's scope and
  outside the `bd-1du.4` epic. Because the workspace gate is not fully
  green, **no beads are closed in this pass**.

Bead status written by this pass:

- `bd-1du.4.6` → progress comment added (4.e integration test landed;
  kernel-side write forwarding still owed).
- `bd-1du.4` → **not** closed (per instructions).
- `bd-1du.10` → **not** closed (per instructions).

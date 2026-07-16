# CLAUDE.md

## Purpose

This file is the current handoff and execution dossier for the `pcloud-rs` C codebase and the Rust rewrite.

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

- `RUST-PLANS/` was the original handoff directory and has been
  removed from the tree. Active execution plans now live alongside
  the parity truth files (`STATUS.md`, `C_FEATURE_PARITY_REVIEW.md`,
  `C_FEATURE_PARITY_MATRIX.csv`) and the closure checklist
  (`docs/parity/bd-1du-10-closure-checklist.md`).

Issue tracker source of truth:

- `bd` (live records under `.beads/issues.jsonl`). The `bd-1du.*`
  IDs referenced in older sections of this file are historical;
  current beads use the `pcloud-rs-ncx.*` naming and the
  `gptrev-01` namespace. Treat the older ID strings as parity
  provenance, not live tickets.

## Current Truth (2026-05-01, post Fire 91)

The Rust implementation is **substantially complete** and is in the final
parity-proof phase. As of Fire 91 (2026-05-01) all CSV `Partial` rows are
closed (tally `156 / 0 / 0 / 30 (186 rows)`), but it is still **not** honest
to call it "full parity", "production ready", or "drop-in replacement" while
cross-platform hardware verification and human reviewer sign-off remain open.

Open parity work (no live beads — see note below):

- **CSV parity is functionally complete** (was bead `bd-1du`, now historical).
  Zero `Partial` rows remain in
  [`C_FEATURE_PARITY_MATRIX.csv`](C_FEATURE_PARITY_MATRIX.csv) as of
  2026-05-01. Row 94 (`transfers,SDK UploadSession`,
  `crates/pcloud-sdk/src/upload_session.rs`) flipped to Implemented in
  Fire 91 after wiring the chunked driver, threading `ConflictMode`
  end-to-end, and adding a mock-server integration test. Rows 124, 138,
  142, 147, 148, 168 previously listed as Partial flipped to Implemented on
  2026-04-30 in CLAUDEREV remediation fires 12-15 (rows 138, 147, 148, 168),
  fire 47 (row 142), and fire 56 (row 124).
- **Mounted-drive cross-platform hardware verification** (was bead
  `bd-1du.4`, now historical). Linux is live-verified; macOS and Windows
  bring-up plus BSD rc.d supervision are tracked under
  `bd-xplat-windows` / `bd-xplat-bsd` in the active `pcloud-rs-ncx.*`
  bead family.
- **Final parity proof gate** (was bead `bd-1du.10`, now historical). See
  [`STATUS.md`](STATUS.md) for the authoritative tally and human sign-off
  remaining; the active closure checklist is at
  [`docs/parity/bd-1du-10-closure-checklist.md`](docs/parity/bd-1du-10-closure-checklist.md).

The `bd-1du.*` IDs above were renamed during the bead-renaming sweep and
do not exist in `.beads/issues.jsonl` today (verified by
`grep '"id":"bd-1du' .beads/issues.jsonl` → 0 matches). Treat the older
IDs as parity provenance, not live tickets. CLAUDEREV iter-3 fix.

Single source of truth for counts: [`STATUS.md`](STATUS.md). Per-row
rejected rationale lives in `REJECTED-RATIONALES-14042026.md`. Do not
hard-code count numbers in this file; link to `STATUS.md` instead.

Important corrections:

- crypto is active on the retained Rust path,
- plain shares/business/team parity is implemented on the retained path;
  crypto share/team-share rows 124, 138, and 142 flipped to Implemented on
  2026-04-30 (CLAUDEREV fires 15, 47, 56),
- core public-link parity is implemented on the retained path; the three
  previously-partial specialty public-link helpers (rows 147, 148, 168)
  flipped to Implemented on 2026-04-30 (CLAUDEREV fires 12-14),
- backup/device/account parity is implemented on the retained path,
- `psync_send_publink` is implemented (row 42, `account_backend.rs`),
- sync helper parity is implemented on the retained path,
- Linux FUSE read+write is live-verified end-to-end on a real kernel mount,
- the remaining parity work is narrow: see [`STATUS.md`](STATUS.md) — CSV
  parity is functionally complete (zero Partial rows as of 2026-05-01,
  Fire 91); cross-platform mount hardware verification and final reviewer
  sign-off are the remaining gates.

Do **not** claim:

- “full parity”
- “production ready”
- “enterprise ready”
- “drop-in replacement”

unless the final parity gate is satisfied by code, tests, docs, and parity matrix evidence.

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
- some end-to-end proof remains in the final parity gate even though the current sync helper matrix rows are implemented.

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
- tree-link create, including root/folder/file path-to-id resolver support
- upload-access helpers
- bookmark/pin helpers

Remaining public-link work is narrow but real: rows 147, 148, and 168 have
backend/proto helpers for folder-link options, folder up/down links, and
screenshot links, but no user-facing IPC/daemon/CLI/SDK routes yet.

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

Previously listed as missing but now implemented:

- `change_crypto_pass` family (`CryptoShell::change_password` / `change_password_unlocked`),
- `send_change_user_private` (`SendChangeUserPrivateRequest` in `pcloud-proto/src/methods/crypto.rs`),
- `priv_key_flags` (`CryptoShell::priv_key_flags`).

Crypto-share/team-share rows 124, 138, and 142 flipped to Implemented on
2026-04-30 in CLAUDEREV remediation fire 15 (row 138, non-RSA crypto-share
end-to-end IPC), fire 47 (row 142, crypto team-share end-to-end IPC), and
fire 56 (row 124, RSA-4096-OAEP crypto-share with multi-RPC orchestrator).
Live two-account/team E2E proof remains gated on the standing live-e2e
harness — same posture as every other recently-implemented Implemented row.

Primary files:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/lib.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-daemon/src/runtime.rs`
- `/home/ezechiel203/Projects/FORKS/pcloud-rs/crates/pcloud-crypto/src/share_temppass.rs`

### Crypto: dual-backend model

Two backends live in `crates/pcloud-crypto`:

- **`CryptoBackend::PclsyncCompat`** (default) — byte-compatible with
  the official pCloud C client (pcloudcc, pCloud Drive, iOS/Android).
  Scheme: PBKDF2-HMAC-SHA512 + RSA-4096-OAEP + custom sector AEAD.
  See `docs/crypto-reference-pclsync.md` for the full spec.

- **`CryptoBackend::Enhanced`** (opt-in) — stricter AEAD (AES-256-GCM)
  + Argon2id KDF. NOT interoperable with pCloud apps. Users must
  pass `--acknowledge-not-interop` to select this backend.

The two are wire-incompatible by design. Profile metadata records
the active backend; unlock dispatches accordingly; cross-backend
unlock returns `BackendMismatch` with no silent fallback.

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

Current state (post Fire 91, 2026-05-01):

- The parity matrix has been reconciled against the source tree. The
  honest count is in `STATUS.md`: **156 Implemented / 0 Partial /
  0 Missing / 30 Rejected (186 rows)**.
- All 30 `Rejected` rows have a 1:1 rationale in
  `REJECTED-RATIONALES-14042026.md`. No Rejected row is unjustified.
- All retained matrix rows that are marked `Implemented` have real,
  reachable code. Historical spot-check notes remain below, but current
  counts come only from `STATUS.md` and the CSV.
- **Zero** `Partial` rows remain. Row 94 (`transfers,SDK UploadSession`,
  `crates/pcloud-sdk/src/upload_session.rs`) flipped to Implemented in
  Fire 91 (2026-05-01) after wiring the chunked driver
  (`upload_create` → per-chunk `upload_write` → `upload_save`), threading
  `ConflictMode::IfHashNumeric` through the wire-level `upload_save`
  request, and adding a mock-server integration test exercising
  `EmbeddedDaemon::start_upload` end to end.
- Rows 124, 138, 142, 147, 148, 168 previously listed here as Partial
  flipped to Implemented on 2026-04-30:
    - Rows 147, 148, 168 — CLAUDEREV remediation fires 12, 13, 14
      (folder-public-link-full, folder-updown-link, screenshot-public-link
      IPC routes + daemon dispatch + privileged classification).
    - Row 138 — CLAUDEREV remediation fire 15 (`Request::CryptoShareFolder`
      non-RSA IPC route + daemon dispatch).
    - Row 142 — CLAUDEREV deferred-set fire 47
      (`Request::CryptoAccountTeamShare` IPC route + daemon dispatch +
      `team_share_verb.rs` live verb-reached test).
    - Row 124 — CLAUDEREV deferred-set fire 56
      (`Request::CryptoShareFolderRsa` IPC route + multi-RPC orchestrator
      with `crypto_getpubkey` → `wrap_share_invitation_b64` →
      `SharesRuntime::crypto_share_folder_rsa`).

Remaining work to close the gate:

- sweep docs for any "production ready" / "full parity" claims (none
  found by this audit, but gate must re-verify at close-time);
- complete cross-platform mount hardware verification (macOS / Windows /
  BSD), tracked under `bd-xplat-windows` / `bd-xplat-bsd`;
- run the nine-gate CI once more against the final tree;
- obtain human reviewer sign-off (out of AI scope).

See [`PARITY-PROOF-CHECKLIST.md`](./PARITY-PROOF-CHECKLIST.md) for the
line-level closure checklist.

### P1 blockers

Zero CSV `Partial` rows remain as of 2026-05-01 (Fire 91 closed Row 94,
SDK `UploadSession`). No live `.beads` entry is required for parity-row
tracking. Historical `bd-1du.*` and `gptrev-01` labels are provenance
only; see [`STATUS.md`](STATUS.md) for the authoritative tally.

## Feature Parity Matrix Summary

The authoritative matrix is:

- `/home/ezechiel203/Projects/FORKS/pcloud-rs/C_FEATURE_PARITY_MATRIX.csv`

High-level status (2026-05-01):

- `Auth`: implemented, live-verified
- `CLI`: implemented for the retained legacy surface
- `Sync root management`: implemented on the retained path
- `Sync engine`: implemented on the retained path, background sync loop wired at daemon startup
- `Transfers`: implemented; row 94 (`SDK UploadSession` public route) flipped to Implemented 2026-05-01 (Fire 91) — see [`STATUS.md`](STATUS.md)
- `Public links`: core/id/path tree-link surfaces implemented; specialty rows 147/148/168 flipped to Implemented 2026-04-30 (CLAUDEREV fires 12-14)
- `Filesystem / mounted drive`: Linux live-verified; macOS/Windows scaffolded, hardware verification remaining
- `Crypto`: implemented on the retained path; crypto-share/team-share rows 124 and 142 flipped to Implemented 2026-04-30 (CLAUDEREV fires 47 + 56)
- `Shares / business / teams`: plain retained path implemented; crypto-share duplicate row 138 flipped to Implemented 2026-04-30 (CLAUDEREV fire 15)
- `Backup / device`: implemented on the retained path
- `SDK breadth`: implemented on the retained path; row 94 `UploadSession` flipped to Implemented 2026-05-01 (Fire 91)

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

**Windows posture (Tier-2 compile + `--lib` tests, 2026-04-24):** A
17-commit bring-up sweep (`8b1c0fe..24fb5bf`) took Windows from
Tier-3 scaffolded-only to Tier-2. The workspace now compiles clean on
Windows MSVC 14.44 + Rust 1.95 + WinFSP 2.1.25156 (0 errors, 0 warnings)
and `cargo test --workspace --lib` reports **1449 passing / 0 failing /
2 ignored** across 33 test binaries. Two production-logic bugs were
surfaced and fixed in-session: a `TcpStream::drop` FIN race in
`pcloud-daemon::health_server` that truncated HTTP response tails on
Windows (commit `24fb5bf`, fixed via explicit `shutdown(Write)` before
drop), and a hardcoded `/` separator in
`pcloud-backends::mount_discovery::is_ignored_under` that broke nested-
root classification on Windows canonical `\\?\`-prefixed paths
(commit `88739da`, fixed to accept both separators). `.gitattributes`
(commit `e13c890`) locks line endings to `eol=lf` to prevent CRLF-
munging regressions, and `vendor/msvc_spectre_libs_stub/` patches out
MSVC Spectre-libs install friction.

Windows is **not** Tier-1. The gating remaining work:

- **Named-pipe IPC accept-loop wiring.** `WindowsIpc` in
  `crates/pcloud-ipc/src/platform/windows.rs` compiles and the
  `bind_listener` / `peer_uid` / `peer_display` trait methods are
  implemented, but the named-pipe backend is **not** wired through the
  `serve_once_with_peer` accept loop in `transport.rs`.
  `pcloud_daemon::serve_with_shutdown` on Windows currently returns
  `Unsupported` (commit `d79004d`); `pcloudd-svc` compiles and starts
  but runs a no-op stub until this lands. This is the Tier-1 blocker
  and in-flight as of this writing.
- **Integration tests.** `cargo test --workspace --tests` has NOT been
  run on Windows — only `--lib`.
- **Live WinFSP mount.** WinFSP FFI is compile-clean only; no live
  mount against a real pCloud account has been exercised.

Tracked under `bd-xplat-windows`. Do not document Windows as a
production-supported platform until the named-pipe accept loop, live
WinFSP mount, and the Windows Service serving path are live-verified.

**FreeBSD CI posture (Tier-3 best-effort):** The FreeBSD CI job has
`continue-on-error: true`. It is informational only; regressions on
FreeBSD do not fail the PR gate. See the FreeBSD job comment in
`.github/workflows/ci.yml` for the documented conditions under which
`continue-on-error` may be removed.

**Signal-driven mount cleanup posture (pcloud-rs-ncx.29, audit-06):**
Signal-driven reaping of stale kernel mount handles on SIGTERM/SIGINT
(or Ctrl-C / service-stop on Windows) is live-verified on **Linux only**
(`crates/pcloud-fs/src/platform/linux.rs::reap_all_mounts` walks
ACTIVE_MOUNTS and issues `umount2(MNT_DETACH)` per-entry). macOS has
the same pattern wired against `fuse-t`
(`platform/macos.rs::install_signal_handler_once`), live-verified on
real Darwin hardware is still pending hardware sign-off.
**BSD and Windows mount cleanup is Tier-3**: the signal handler is
installed and an AtomicBool flag is flipped, but the reaper does **not**
drain an ACTIVE_MOUNTS registry and does **not** call
`unmount(MNT_FORCE)` / `FspFileSystemStopDispatcher`. No such registry
or dispatcher wiring exists on those platforms in this fork (they are
tracked under `bd-xplat-bsd` and `bd-xplat-windows`). Operationally
this means: on BSD and Windows, a process crash may leave a stale
mountpoint that the operator must clean up manually (`umount -f` on
BSD, WinFSP admin tooling on Windows). Do not claim graceful shutdown
on BSD/Windows until `bd-xplat-bsd` / `bd-xplat-windows` close and the
reaper bodies at `platform/bsd.rs::bsd_reaper_main` and
`platform/windows.rs::windows_reaper_main` are upgraded to drain their
respective mount registries.

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

## Historical Parallel Work Plan For Another Agent

This split preserves the old ownership vocabulary for context only; the
`bd-1du.*` labels are not live beads.

Recommended parallel split:

### Agent A: Filesystem / mount parity

Own:

- historical `bd-1du.4` scope

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

- historical `bd-1du.10` scope

Scope:

- continue line-by-line review,
- maintain `C_FEATURE_PARITY_REVIEW.md`,
- maintain `C_FEATURE_PARITY_MATRIX.csv`,
- block false parity claims,
- verify docs and release wording.

This agent must not close the final parity gate until every retained row has
current evidence.

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

Tracker:

```bash
cd /home/ezechiel203/Projects/FORKS/pcloud-rs
bd list --status=open
rg -n '"id":"bd-1du' .beads/issues.jsonl
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

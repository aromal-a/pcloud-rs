# Partial Transfer Resume (Uploads H5, Downloads H6)

## 1. Purpose

Two crash-safe mechanisms in the Rust rewrite that the legacy C client
did not have in a durable form:

- **Upload resume (H5)** — NDJSON sidecar written next to each staged
  payload; replayed on daemon start, cross-checked with the server via
  `upload_status`, classified into a **seven-variant `ResumeOutcome`**.
- **Download resume (H6)** — HTTP `Range`-aware download at
  `pcloud-proto::fetch_download_resumable`, with `.part` staging,
  atomic rename on success, and prefix re-hash before trust.

This page lets an operator reason about what happens when the daemon
is killed mid-transfer, and tells a contributor how to migrate more
daemon sites onto these entry points.

All file paths are relative to ``.

## 2. Prereqs

- `pcloud-daemon` at the version that includes H5 wiring (bootstrap
  replay + mount-runtime re-replay; verified in
  `crates/pcloud-daemon/src/bootstrap.rs` and
  `crates/pcloud-daemon/src/mount_runtime.rs`).
- For uploads: authenticated session; staging directory writable at
  `0700` with sidecars at `0600`.
- For downloads: destination directory writable; enough free space
  for `<dest>.part` (equal to the final file size).
- Native JSON parser for status selectors (`jq`, `ConvertFrom-Json`,
  etc.).

## 3. Conceptual background

### Scope and honesty statement

Resume is not one feature — it is two independent subsystems sharing
a design philosophy: **write the minimum crash-safe breadcrumb,
re-derive everything else by asking the server.**

- **Upload resume (H5)** is **wired into the daemon.** Both bootstrap
  and the mount runtime call `replay_upload_sidecars` on start-up.
  Any sidecar left by a previous daemon is evaluated and either
  resumed, trimmed, or retired.
- **Download resume (H6)** is a **library API on `pcloud-proto`**
  (`fetch_download_resumable`). It is **not yet the default path** for
  every daemon-side download. Existing callers of
  `fetch_download_verified` need to be migrated one site at a time.
  Until then, a daemon-mediated download of a large file interrupted
  by a daemon restart will **re-fetch from byte 0**, not from the
  last committed offset.

If you are deciding whether it is safe to kill `pcloudd` mid-transfer:
uploads survive that, downloads launched through
`fetch_download_verified` do not, downloads launched through
`fetch_download_resumable` do.

### Upload resume (H5)

The pCloud upload API is three-phase:

1. `upload_create` — allocates a server-side upload id.
2. `upload_write` — pushes bytes at a given offset (one or more calls).
3. `upload_save` — atomically materialises the uploaded content.

If the daemon dies between phases 2 and 3, the server retains the
partial upload for a grace period, but the *client* has no built-in
memory of which upload id belonged to which local inode, how far
through the file it was, or whether the last `upload_write` was
acknowledged. H5 adds that memory, crash-safely, without trusting any
in-memory state across a restart.

#### Sidecar layout

For every in-flight upload the daemon writes a sidecar next to the
staged payload under the staging directory:

```
<runtime-staging>/ino-<inode>.upload-progress
```

NDJSON: one JSON record per line, append-only during a live upload,
rewritten in place on heartbeat updates. Each record carries at
minimum:

- the local inode number (primary key),
- the server-assigned upload id,
- the acknowledged byte offset,
- the SHA-256 of the prefix up to that offset,
- a monotonic `heartbeat_unix_secs` timestamp updated on every
  successful `upload_write` acknowledgement.

All sidecar writes go through **write-temp → `fsync(file)` → rename →
`fsync(dir)`**. A crash at any point leaves either the previous
consistent sidecar (rename not committed) or the new consistent
sidecar (rename committed and both `fsync` calls returned). A
subsequent replay never observes a half-written state.

#### The `upload_status` probe

Replay does not trust the sidecar unconditionally. On replay the
daemon calls an `upload_status` protocol method and cross-checks:

- If server offset **equals** sidecar offset → resume at that
  offset.
- If server offset **exceeds** sidecar offset → sidecar is behind;
  advance to the server offset. No bytes re-uploaded.
- If server offset **is less than** sidecar offset → server trimmed
  our tail; rewind and re-send from there.
- If the upload id is gone (expired / GC’d) → mark for a fresh
  `upload_create`.

This is always a network round trip at start-up. Deliberate: the
sidecar is an *optimistic* local cache; the server is the only source
of truth for durably-persisted bytes.

#### `ResumeOutcome` — the seven-variant taxonomy

`replay_upload_sidecars` in `crates/pcloud-fs/src/write_path.rs`
emits one `ResumeOutcome` value per sidecar it evaluates:

1. **`Resumed`** — sidecar and server agree; upload continues from
   the recorded offset. Next `upload_write` is issued at that offset.
2. **`ServerAhead`** — server offset is larger than sidecar; sidecar
   is rewritten to match the server. No bytes re-uploaded.
3. **`SidecarTrimmed`** — server offset is smaller than sidecar;
   sidecar is rewound to the server offset, bytes past that point
   are re-sent.
4. **`Expired`** — server no longer recognises the upload id; the
   sidecar is retired and a fresh `upload_create` is queued.
5. **`Stalled`** — the sidecar heartbeat is older than the stall
   timeout AND the server has no meaningful progress. Upload is
   aborted with a retryable error. Stall timeout defaults to
   **10 minutes** (`DEFAULT_HEARTBEAT_TIMEOUT` in
   `write_path.rs::846`).
6. **`Unparseable`** — sidecar on disk is corrupt or from an
   incompatible version. Left in place for operator inspection; a
   warning is logged. Diagnostic-only; the upload state is not used.
7. **`BackendError`** — `upload_status` itself failed (network, auth,
   server error). Sidecar is not modified; re-evaluated on the next
   replay tick.

Callers that want outcomes without acting (boot-time diagnostics)
can use `enumerate_upload_sidecars`, which parses sidecars but does
not talk to the server.

#### Where replay runs

- `crates/pcloud-daemon/src/bootstrap.rs` — replay pass at daemon
  start-up, after the vault is loaded and before the mount comes up.
  `Resumed` / `ServerAhead` / `SidecarTrimmed` outcomes are handed
  to the active upload backend so transfers resume.
- `crates/pcloud-daemon/src/mount_runtime.rs` — re-runs replay when
  the mount is (re)activated, because a mount may come up later than
  the daemon itself (for example after interactive `crypto start`
  or `pcloudc mount`).

Both sites treat `BackendError` as non-fatal and reschedule; both
treat `Expired` / `Stalled` as terminal for the current upload and
surface them through the normal transfer-error path.

### Download resume (H6)

HTTP downloads against the pCloud signed-URL infrastructure are a
single `GET` whose response body is the file. If the connection dies
halfway through, the cleanest recovery is to reissue `GET` with a
`Range: bytes=N-` header. Two prerequisites:

- the server must honour `Range`,
- the client must prove that bytes already on disk are correct before
  trusting them as the resume prefix.

`fetch_download_resumable` lives at
`crates/pcloud-proto/src/http_download.rs`. Contract:

- The target is written to `<dest>.part` while in flight; atomically
  renamed to `<dest>` only on successful, hash-verified completion.
- On entry, if `<dest>.part` exists, its length `N` is the resume
  offset. Request issued with `Range: bytes=N-`.
- The on-disk prefix is re-hashed against the expected-final-hash's
  prefix before any new bytes are appended. This is a **full-length
  rescan of the `.part` file**. Cost is **O(file-size)** and is
  documented on the function: resuming a half-downloaded 10 GiB
  archive reads 5 GiB from disk before the first new byte is
  fetched.
- If the prefix hash does not match the expected hash (the file on
  the server changed, or `.part` was corrupted between runs), the
  `.part` is deleted and the download restarts from byte 0. **No
  partial data is ever silently concatenated.**
- If the server responds to `Range` with `200 OK` instead of
  `206 Partial Content` — typically because the signed URL is routed
  through a CDN that does not advertise `Accept-Ranges: bytes` — the
  client **falls back to a full-file download**, discarding the old
  `.part`. The fallback is unconditional; a `200` response is never
  spliced onto a `.part` prefix.

The returned `DownloadOutcome` enum surfaces which branch ran:

- **`Fresh`** — no `.part` existed, or the server returned `200`
  without a usable `Accept-Ranges: bytes`; full download issued.
- **`Resumed`** — `.part` existed, prefix hash matched, server
  returned `206 Partial Content` starting at the requested offset.
- **`FullReplace`** — `.part` existed but prefix hash mismatched, or
  server forced a `206 → 200` fallback; `.part` was replaced.

In all three terminal states the final on-disk file is
hash-verified end-to-end before the rename; no unverified byte
reaches `<dest>`.

## 4. Step-by-step procedure (operator)

### 4.1 Check sidecar state on a running daemon

```bash
# Staging dir is under the runtime directory:
ls -la "$XDG_RUNTIME_DIR/pcloud-rs/staging"/*.upload-progress

# Check status (human):
pcloudc status sync          # inline transfer summary
pcloudc doctor               # bundles upload sidecar health

# Machine-readable (selector):
pcloudc doctor --json \
  | jq '.checks[] | select(.id == "transfers.sidecars")'
```

### 4.2 Kill the daemon mid-upload (safe)

```bash
systemctl --user stop pcloud-rs-daemon     # SIGTERM → drain
# Inspect surviving sidecars:
ls -la "$XDG_RUNTIME_DIR/pcloud-rs/staging"/*.upload-progress

systemctl --user start pcloud-rs-daemon    # bootstrap runs replay
# Verify:
pcloudc doctor --json \
  | jq '.checks[] | select(.id == "transfers.sidecars") | .detail.outcomes'
# Example output:
# { "Resumed": 12, "ServerAhead": 1, "SidecarTrimmed": 0,
#   "Expired": 0, "Stalled": 0, "Unparseable": 0, "BackendError": 0 }
```

### 4.3 Watch for `Stalled` sidecars

```bash
# Stalled sidecars mean an upload has been idle past the 10-minute
# heartbeat. They are flagged in the daemon log and in doctor.
journalctl --user -u pcloud-rs-daemon \
  | grep -E 'Stalled|sidecar.stalled'
```

## 5. Verification

A resume path is "healthy" when:

- `pcloudc doctor --json | jq '.checks[] | select(.id ==
  "transfers.sidecars") | .level'` returns `"ok"`,
- no `Unparseable` outcomes on boot — unparseables are **diagnostic
  signals**, not recovery events,
- no growth in `BackendError` counts across multiple replay ticks
  (one or two are fine; sustained failure means the network / auth
  path is broken),
- for downloads: after resume, the final `blake3`/`sha256` of
  `<dest>` matches the expected-final-hash argument; the file is
  present at `<dest>`, the `.part` is gone.

## 6. Rollback

There is no explicit "undo resume" — the subsystems themselves are
idempotent. The recovery actions are:

- **Operator re-queue.** If an upload lands in `Expired` repeatedly,
  delete the staged payload **and** the sidecar together, then
  re-queue the file for upload.
- **Start fresh.** For a single broken download, delete `<dest>.part`
  and re-issue the download through the same call site.
- **Clean staging.** To force a full reset, drain the daemon, remove
  every `*.upload-progress` sidecar under the staging directory,
  and restart. All in-flight uploads restart from byte 0.

## 7. Tradeoffs / tuning

| Knob                                        | Default    | Tradeoff                                                                |
|---------------------------------------------|------------|-------------------------------------------------------------------------|
| `DEFAULT_HEARTBEAT_TIMEOUT`                 | 10 min     | Shorter → more `Stalled` false positives; longer → slower abort.        |
| Prefix re-hash on resume (H6)               | mandatory  | Cost is O(file size); non-negotiable — removing it breaks integrity.    |
| `fetch_download_resumable` vs `fetch_download_verified` | caller-chosen | Resumable carries `.part` staging + range logic; verified is simpler but not crash-safe. |
| Sidecar fsync frequency                     | per-ack    | More fsyncs = stronger durability, higher write amplification.          |
| Upload parallelism                          | caller     | More parallel uploads = more sidecars = more replay work at boot.       |

## 8. Common failure modes

1. **Worked scenario: daemon crash mid-upload.**
   - Symptom: `systemctl --user status pcloud-rs-daemon` reports
     abnormal exit; sidecars remain under staging.
   - Recovery: restart the daemon. Bootstrap runs replay; outcomes
     should be dominated by `Resumed` / `ServerAhead`. Watch
     `doctor` for `Stalled` entries — those are the ones needing
     attention.

2. **Worked scenario: network blip mid-download.**
   - Symptom: the active HTTP stream disconnects; `<dest>.part`
     grows to N bytes and stops.
   - Recovery (resumable call site): rerun the operation. H6 sees
     `<dest>.part` of length N, issues `Range: bytes=N-`, re-hashes
     the prefix, receives `206` and appends. `DownloadOutcome ==
     Resumed`.
   - Recovery (verified-only call site): the download restarts from
     byte 0 on retry. Known gap; migrate the call site.

3. **Worked scenario: force-restart handling.**
   - Symptom: `kill -9 pcloudd` (NOT a graceful drain) — sidecars
     may lag the last ack by one heartbeat cycle.
   - Recovery: on next boot, `ServerAhead` is the common outcome;
     the sidecar rewrites itself to match the server. No data loss.

4. **`Unparseable` sidecars after a version bump.**
   - Cause: sidecar format changed between versions.
   - Fix: delete the unparseable sidecar file **and** the
     associated staged payload together, then re-queue the upload.
     Never try to hand-edit a sidecar.

5. **`BackendError` dominates replay outcomes.**
   - Cause: auth not re-established yet; network offline;
     `upload_status` rejected by the server for a revoked token.
   - Fix: `pcloudc doctor --json | jq '.checks[] | select(.id ==
     "auth")'`; re-authenticate if necessary. Replay retries on
     the next tick — no operator action is typically needed once
     auth is restored.

## 9. Security / compliance notes

- **Sidecars contain no secrets.** Only an upload id, an offset, a
  digest, and a timestamp. The upload id is a server-issued handle;
  it does not grant account access on its own. Sidecars inherit
  `0700` on the staging directory and are created `0600`.
- **Sidecar integrity is not authenticated.** A local attacker with
  UID access could corrupt a sidecar — but they already have UID
  access, which is inside the daemon’s trust boundary. The corrupt
  sidecar is classified `Unparseable` and does not poison recovery.
- **Download prefix re-hash is mandatory.** If an attacker tampers
  with `<dest>.part`, the re-hash fails and the download restarts
  from byte 0. There is no "trust-the-prefix" shortcut.
- **No byte reaches `<dest>` unverified.** End-to-end final hash is
  checked before the rename in every `DownloadOutcome` branch.
- **`0700` / `0600` permissions are enforced** — do not loosen them;
  the daemon refuses to start with a permissive staging directory
  (`doctor` check `staging.mode_invalid`).

## 10. Contributor notes

- Prefer `fetch_download_resumable` when adding new daemon transfer
  paths.
- When porting an existing `fetch_download_verified` call site:
  - switch the destination handling to a `.part`-staged path,
  - add the expected-final-hash argument so the resume prefix check
    can run,
  - extend the call site tests to cover all three
    `DownloadOutcome` branches (`Fresh`, `Resumed`, `FullReplace`).
- H5 is ready to add more callers: if you write a new upload-like
  path, use `replay_upload_sidecars` as the recovery seam and emit
  the same `ResumeOutcome` taxonomy so `doctor` aggregates stay
  useful.

## 10.1 Upload via mounted drive (bd-1du.4.6 — Linux, pre-alpha)

Status: **landed on the direct-shim path**
(`PcloudFsShim` + `MountService::mount_fuser`). The `BoxedFuserShim` /
`FuserShim<A>` dyn-trait shim is **still read-only** by design; writable
mounts must be composed through `PcloudFsShim` directly. This matches
the `#[must_use]` construction in `crates/pcloud-daemon/src/mount_runtime.rs`.

### What the recipe proves

Writing a file through the kernel VFS against a `PcloudFsShim`-backed
FUSE mount:

1. stages bytes on-disk under `<state_dir>/stage/<blob-name>`,
2. journals `Create` / `Write` / `FlushBarrier` records under
   `<state_dir>/journal.bin` with the `fsync(file)+fsync(dir)` barrier
   described in §3,
3. finalizes via `upload_file` on the wired transport at `close(2)` /
   `fsync(2)` (whole-file finalize; chunked `upload_write`
   pipelining is still `TODO(bd-1du.4.6)` in `write_path.rs`),
4. survives an unmount → remount cycle: once the server has absorbed
   the upload, the next cold mount reads the file back byte-identical
   through the kernel VFS via the `ProtoFuseAdapter` read path.

The integration proof is
`crates/pcloud-fs/tests/fuse_write_path_live.rs::write_unmount_remount_readback_byte_identical`
(gated on `PCLOUD_LIVE_E2E=1` or `PCLOUD_FUSE_TEST=1`). It drives the
whole loop through a real Linux FUSE kernel mount.

### Supported write-side FUSE ops on `PcloudFsShim`

Wired through `WritePathService`:

- `create` — allocates an in-progress write handle + journals a
  `JournalOp::Create`. Publishes the new entry into the adapter's
  local metadata cache so subsequent `lookup` / `readdir` succeed
  before the upload finalizes.
- `write` — appends bytes into the per-inode staging blob, journals a
  `JournalOp::Write`, bumps the local size published to `getattr` so
  in-flight reads see monotonically-growing length.
- `flush` / `fsync` — forces a finalize via `upload_file` once the
  staging blob is at rest. `flush` is a no-op on read-only handles.
- `release` — runs the final flush, tears down the write handle, and
  invalidates the parent directory and file cache entries so the
  next `readdir` pulls the server-assigned `file_id`.
- `setattr(size)` — truncate, journaled via `JournalOp::Truncate`.
- `unlink` — propagates to the remote via `unlink_remote`, forgets
  the local metadata cache entry.
- `rename` — propagates to the remote via `rename_remote`, carries
  any cached attribute over to the destination path.
- `mkdir` / `rmdir` — forwarded to the `FolderBackend` directly
  (no staging — directories are metadata-only on pCloud).

### Operator recipe

```bash
# 1. Bring up a writable mount through the daemon.
pcloudc mount --read-write /mnt/pcloud

# 2. Write a file — the kernel VFS drives FUSE `create`/`write`/
#    `release` through the shim, and the release triggers an
#    `upload_file` call on the wired transport.
cp ~/report.pdf /mnt/pcloud/Documents/

# 3. Force a flush before unmount. `sync` targets the whole mount;
#    for a single file use `fsync(2)` equivalent (e.g. python
#    `os.fsync(f.fileno())`).
sync /mnt/pcloud

# 4. Check upload-sidecar state if you want proof a finalize did
#    or did not complete — sidecars live under:
ls -la <state_dir>/stage/*.sidecar 2>/dev/null

# 5. Unmount — flushes all pending handles, journals FlushBarrier.
pcloudc unmount /mnt/pcloud
```

### Interaction with H5 upload resume

The write path inside `PcloudFsShim` writes the same
`ResumeOutcome`-emitting sidecars as the standalone uploader once
chunked `upload_write` pipelining lands. Until then, finalize happens
through `upload_file`; a crash after `FlushBarrier` but before the
upload ack is safe because the next daemon start replays the journal
and re-emits the upload. The existing replay path
(`crates/pcloud-daemon/src/bootstrap.rs`,
`crates/pcloud-daemon/src/mount_runtime.rs`) already calls
`replay_upload_sidecars` on mount (re)activation, so no additional
operator step is required.

### Honesty statement

This is **pre-alpha**. What is **not** yet in place:

- chunked `upload_write` pipelining for sustained multi-GiB writes
  (tracked under `TODO(bd-1du.4.6)` in `write_path.rs`);
- writable mounts through the object-safe `BoxedFuserShim` — the
  dyn-trait shim stays read-only because `WritePathService<U>` is
  generic over the concrete upload backend. Follow-up is explicitly
  noted in `crates/pcloud-fs/src/platform/linux.rs`;
- macOS and Windows writable mounts (scaffolding only);
- final `bd-1du.10` parity gate closure.

Until `bd-1du.10` closes, do **not** claim "production-ready" or
"drop-in replacement" for the mounted-drive surface.

## 11. Cross-references

- `crates/pcloud-fs/src/write_path.rs` — H5 sidecar writer +
  `ResumeOutcome` variants.
- `crates/pcloud-daemon/src/bootstrap.rs` — replay at boot.
- `crates/pcloud-daemon/src/mount_runtime.rs` — replay on mount
  (re)activation.
- `crates/pcloud-proto/src/http_download.rs` — H6 download +
  `DownloadOutcome`.
- `crates/pcloud-proto/tests/http_download_integrity.rs` —
  integration tests for the three download branches.
- [Upgrade](./upgrade.md) — sidecar behavior across a graceful drain.
- [Runbook](./runbook.md) — playbooks for stuck transfers.
- [CLI reference — `doctor`](../reference/cli.md#doctor).

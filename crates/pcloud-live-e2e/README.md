# pcloud-live-e2e

Live-API verification harness for the pcloud-rs Rust rewrite.

These tests execute real requests against `api.pcloud.com` (or the
configured production endpoint). They are **off by default** and must
never run in an untrusted CI environment or with shared credentials.

## Safety contract

- Every test is marked `#[ignore]`. A normal `cargo test` (any profile,
  any workspace invocation) **will not** run them.
- Every test additionally short-circuits unless
  `PCLOUD_LIVE_E2E=1` is set at runtime, so even
  `cargo test --workspace -- --ignored` is a no-op without the gate.
- Credentials are read **only** from environment variables. They are
  never logged, never written to disk, and never embedded in source.
- Each test runs under a unique temp directory that is deleted on
  `Drop`. No auth tokens, vaults, or sync state survive the run.
- Every asserted response is scanned for the live password/token/TFA
  values that were fed in — a leak fails the test.
- Mutating flows clean up the objects they create (uploads, public
  links, sync roots).

## Running locally

```bash
export PCLOUD_LIVE_E2E=1

# Either a bearer token (preferred):
export PCLOUD_TEST_TOKEN='...'
# OR username + password (+ optional TFA for 2FA accounts):
export PCLOUD_TEST_USER='you@example.com'
export PCLOUD_TEST_PASSWORD='...'
export PCLOUD_TEST_TFA_CODE='123456'            # optional
export PCLOUD_TEST_RECOVERY_CODE='...'          # optional

# Optional: a remote folder the harness can write into ("/" by default).
export PCLOUD_TEST_SCRATCH='/live-e2e-scratch'

# Optional: crypto password for the crypto harness.
export PCLOUD_TEST_CRYPTO_PASSWORD='...'

# Optional: a remote path the harness is allowed to publish a link for.
export PCLOUD_TEST_PUBLIC_LINK_PATH='/live-e2e-scratch/shareable.txt'

cd .
cargo test -p pcloud-live-e2e -- --ignored --test-threads=1
```

Always pass `--test-threads=1` so concurrent runs don't race on a shared
remote scratch folder.

## Per-family test binaries

| Binary                | Feature family covered                                                       |
| --------------------- | ---------------------------------------------------------------------------- |
| `transfers`           | upload_create/write/save, getfilelink, download round-trip                   |
| `auth_lifecycle`      | login (password+token), logout, session-status, durable-persistence opt-in,  |
|                       | vault/dir permission drill (0600/0700 on Unix)                               |
| `sync_roots`          | add (Full / UploadOnly / DownloadOnly) + list + change-type + pause + resume |
|                       | + remove + remove-idempotence probe                                          |
| `public_links`        | create-file-link + list + change-expire + change-password + delete (by id    |
|                       | and by code) + optional folder-link cycle, with field-selector probes        |
|                       | against the response shape                                                   |
| `snapshot_pipeline`   | snapshot create (default zstd) → verify (SHA3 round-trip) → prune;           |
|                       | optional GPG variant skipped unless `gpg` + `PCLOUD_TEST_GPG_RECIPIENT`      |
| `integrity_sweeper`   | IntegrityStatus probe + IntegrityRunOnce against a seeded sync root:         |
|                       | asserts mismatches=0, monotone counters, audit_drops=0                       |
| `field_selectors`     | bare-field and dotted-path probes on userinfo, session-status, sync-list,    |
|                       | list-public-links (covers both JSON and legacy `key=value` response shapes)  |
| `shares`              | ShareFolder invite + list outgoing + cancel + modify/remove probes;          |
|                       | requires `PCLOUD_TEST_PEER_USER` (single-account flow, no cross-account      |
|                       | handshake yet)                                                               |
| `crypto`              | crypto setup/unlock/status/mkdir/lock/re-unlock lifecycle; requires          |
|                       | `PCLOUD_TEST_CRYPTO_PASSWORD`                                                |
| `snapshot_prune`      | seed 10 fake snapshots spanning ~8 weeks, dispatch GFS prune with            |
|                       | retention_days=7, assert keep/drop set matches GFS bucketing semantics       |
| `mount_linux`         | mount via IPC + readdir + cat first file + unmount (Linux only); requires    |
|                       | `PCLOUD_FUSE_TEST=1` + `/dev/fuse` + credentials                            |
| `rate_limit`          | burst 10 Expensive requests, assert rate-limiter returns Conflict with       |
|                       | category label + retry-after hint                                            |
| `drain`               | drain state machine: Running -> Draining -> Stopped via `begin_drain()` +    |
|                       | `mark_stopped()`; InFlightGuard RAII accounting; no backend credentials      |
|                       | required                                                                     |

Each binary is self-contained and can be run in isolation, e.g.:

```bash
cargo test -p pcloud-live-e2e --test transfers -- --ignored
cargo test -p pcloud-live-e2e --test auth_lifecycle -- --ignored
cargo test -p pcloud-live-e2e --test sync_roots -- --ignored
cargo test -p pcloud-live-e2e --test public_links -- --ignored
cargo test -p pcloud-live-e2e --test snapshot_pipeline -- --ignored
cargo test -p pcloud-live-e2e --test integrity_sweeper -- --ignored
cargo test -p pcloud-live-e2e --test field_selectors -- --ignored
cargo test -p pcloud-live-e2e --test shares -- --ignored
cargo test -p pcloud-live-e2e --test crypto -- --ignored
cargo test -p pcloud-live-e2e --test snapshot_prune -- --ignored
cargo test -p pcloud-live-e2e --test mount_linux -- --ignored
cargo test -p pcloud-live-e2e --test rate_limit -- --ignored
cargo test -p pcloud-live-e2e --test drain -- --ignored
```

Extra environment variables consumed by the new binaries:

| Variable                       | Used by              | Effect                                                        |
| ------------------------------ | -------------------- | ------------------------------------------------------------- |
| `PCLOUD_TEST_GPG_RECIPIENT`    | `snapshot_pipeline`  | Key id / email for the GPG-envelope snapshot create/verify    |
|                                |                      | round-trip. When unset, that single test soft-skips.          |
| `PCLOUD_TEST_PEER_USER`        | `shares`             | Email of a second pCloud account to receive the share invite. |
|                                |                      | When unset, the share test soft-skips.                        |
| `PCLOUD_TEST_CRYPTO_PASSWORD`  | `crypto`             | Crypto passphrase for the test account. When unset, the       |
|                                |                      | crypto test soft-skips.                                       |
| `PCLOUD_FUSE_TEST`             | `mount_linux`        | Set to `1` to opt in to FUSE mount tests. Requires Linux +   |
|                                |                      | `/dev/fuse` + credentials.                                   |

All new binaries honour the same gate (`PCLOUD_LIVE_E2E=1`) and secret-
leak rules as `transfers`; every asserted response is scanned for the
live credential values before any equality check runs.

## What this harness does NOT cover yet

- Two-account cross-account share acceptance (requires a second complete
  credential triplet). The `shares` binary covers single-account invite +
  cancel today.
- Backup create/delete, device stop/delete. Blocked on `bd-1du.8`.
- FUSE write-path end-to-end (write + remount + readback). The
  `mount_linux` binary covers mount + readdir + cat + unmount; full
  write-path proof lives in `pcloud-fs/tests/`.
- Crypto password rotation (`change_crypto_pass`). Requires email
  confirmation code delivery which is not programmatically addressable.

These rows must remain `Partial` in
`C_FEATURE_PARITY_MATRIX.csv` until the corresponding harness
binary is added here.

## CI guidance (not yet wired)

**Do not** wire this crate into the default `cargo test` CI matrix.

Recommended nightly job:

```yaml
# .github/workflows/live-e2e-nightly.yml (template — NOT committed yet)
name: live-e2e-nightly
on:
  schedule:
    - cron: '0 3 * * *'   # 03:00 UTC daily
  workflow_dispatch: {}

jobs:
  live:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    concurrency:
      group: live-e2e-singleton
      cancel-in-progress: false
    environment: live-e2e           # GitHub environment holding the secrets
    env:
      PCLOUD_LIVE_E2E: '1'
      PCLOUD_TEST_TOKEN:            ${{ secrets.PCLOUD_TEST_TOKEN }}
      PCLOUD_TEST_SCRATCH:          ${{ vars.PCLOUD_TEST_SCRATCH }}
      PCLOUD_TEST_CRYPTO_PASSWORD:  ${{ secrets.PCLOUD_TEST_CRYPTO_PASSWORD }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Build workspace
        working-directory: 
        run: cargo check --workspace
      - name: Run live-e2e
        working-directory: 
        run: cargo test -p pcloud-live-e2e -- --ignored --test-threads=1
```

Hard requirements before turning this on:

1. Credentials belong to a **dedicated test account** whose only purpose
   is this harness. Never use a developer's personal account.
2. The GitHub Environment gating `live-e2e` must require manual approval
   and restrict which branches may deploy into it (main/release only).
3. The job must run as a singleton (`concurrency.group`) so concurrent
   runs can't race on the shared scratch folder.
4. Failures must page the Rust rewrite rotation, not the generic CI
   channel, so credential issues are handled out-of-band.
5. Token rotation: configure `PCLOUD_TEST_TOKEN` with the narrowest
   scope the backend allows, and rotate on a documented cadence.

Until all five are satisfied the harness runs locally only.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

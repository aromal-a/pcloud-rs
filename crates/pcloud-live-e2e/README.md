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
| `shares`              | Single-account invite/list/cancel/modify/remove probes with peer email       |
| `shares_a_to_b`       | Two-account invite, accept, bilateral visibility, revoke, and cleanup        |
| `crypto`              | crypto setup/unlock/status/mkdir/lock/re-unlock lifecycle; requires          |
|                       | `PCLOUD_TEST_CRYPTO_PASSWORD`                                                |
| `snapshot_prune`      | seed 10 fake snapshots spanning ~8 weeks, dispatch GFS prune with            |
|                       | retention_days=7, assert keep/drop set matches GFS bucketing semantics       |
| `mount_linux`         | upload fixture + mount via IPC + exact mounted read + unmount + cleanup       |
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

Additional binaries cover backup lifecycle, live sync-loop reconciliation,
tree public links, `upload_writefromfile`, TFA, team-share verbs, two-account
share acceptance/revocation, Windows liveness, fleet mTLS, and guarded account
or crypto-password mutations. The source under `tests/` is the authoritative
inventory.

## CI and release gates

The default workspace test run compiles these binaries but does not execute
their ignored tests. `.github/workflows/ci.yml` runs the credentialed suite on
weekly/manual triggers. `.github/workflows/release.yml` hard-gates a release on
dedicated-account transfer/public-link and two-account share round trips, plus
a credentialed Linux mount fixture read on the native FUSE runner.

Release-selected tests receive `PCLOUD_RELEASE_GATE=1`. In that mode missing
credentials, authentication/TFA failures, unavailable share acceptance,
mount refusal, read failure, and fixture-cleanup failure are test failures;
they cannot be converted to advisory skips.

Operational requirements:

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

The current repository cannot prove those secrets/environments are configured;
only a retained successful release-commit run is qualification evidence.

---

See also: [mdBook crate map](../../docs/book/src/architecture/crate-map.md).

# CLAUDEREV Deferred-Set Plan

Date: 2026-04-30
Source: `CLAUDEREV/REMEDIATION-COMPLETE.md` "What remains externally-blocked" → deferred sub-steps subsection.
Driver: cron `*/3 * * * *` (every 3 min, session-scoped).

This plan picks up the six items the original 36-fire campaign closed as
`ACKNOWLEDGED-DEFERRED` because each was multi-fire scope or
required a design decision that exceeded a single fire's budget. The
campaign is now resourced to attack them one at a time.

Items are ordered by **(blast-radius × prerequisite-chain)** ascending —
smaller, more contained items first so the loop accumulates wins and
unblocks later items.

The user has confirmed the following live resources are available:
- Two live pCloud accounts (`A` + `B`) via `.env`.
- Crypto password for account A.

Plus open questions on hardware / external services — see the companion
"Non-AI dependencies" enumeration in the loop's first chat turn.

---

## Item D1 — Page-cache generalisation (P7.1 follow-up)

- **Files:** `crates/pcloud-cache/src/page_cache.rs`, `crates/pcloud-fs/src/page_cache.rs`, `crates/pcloud-fs/src/read_path.rs`, `crates/pcloud-fs/src/fuse_adapter.rs`.
- **Fix:** make one `PageCache<K>` generic over the key type. Approach: take `pcloud_fs::page_cache::PageCache` (the more capable one with typed `PageKey`, `PageCacheStats`, `invalidate_file`) and parameterise it on the key. Re-implement `pcloud_cache::page_cache::PageCache` as a thin alias `PageCache<String>` that drops `invalidate_file` (which has no meaning for string keys). Delete the old standalone implementation.
- **Decomposition:**
  - **D1.1** (this fire family) Introduce `PageCache<K>` generic in `pcloud-fs::page_cache`. Re-export the existing `PageCache = PageCache<PageKey>` alias so all current callers keep compiling.
  - **D1.2** Migrate `pcloud_cache::page_cache::PageCache` to `pub type PageCache = pcloud_fs::page_cache::PageCache<String>`; update `read_path.rs` if the borrow-checker disagrees about API differences.
  - **D1.3** Delete the now-unused `pcloud-cache/src/page_cache.rs` body; the crate's `lib.rs` re-exports the canonical type from `pcloud-fs`.
- **Verification:** `cargo test -p pcloud-fs --lib page_cache`, `cargo test -p pcloud-cache`, the page-cache benchmark `cargo bench -p pcloud-fs --bench page_cache --no-run`.
- **Acceptance:** symbol `PageCache` resolves to a single canonical generic implementation; the rustdoc cross-references in both crates can be removed.

## Item D2 — `AccountChangePassword` round-trip (P5.2 follow-up)

- **Files:** `crates/pcloud-live-e2e/tests/account_utility_destructive.rs`; possibly a new helper in `crates/pcloud-live-e2e/tests/common/mod.rs`.
- **Fix:** implement the safe `current → temp → current` round-trip with a marker-file recovery pattern that survives `cargo test` invocations.
- **Design:**
  - Marker file path: `${TMPDIR}/pcloud-rs-acp-marker-${user_email_sha256}`. SHA-256-derived filename so the marker is keyed to the account, not the test process.
  - Marker file content: JSON `{"original": "<password>", "temp": "<password>", "phase": "rotated_to_temp"}`. Both values are RedactedString-equivalent — the file is `0600` and gone after a clean run.
  - **Test flow:**
    1. On entry, read marker if present. If the recorded `phase = rotated_to_temp`, use `temp` as current and roll forward (rotate back to original, then delete marker).
    2. Fresh start: authenticate with env-supplied original. Generate `temp = "claudereV-rotation-temp-{nonce}"`. Write marker `phase = rotated_to_temp`. Dispatch `AccountChangePassword{ current: original, new: temp }`. Update marker `phase = rotated_to_temp`.
    3. Re-authenticate with `temp`. Dispatch `AccountChangePassword{ current: temp, new: original }`. Delete marker.
  - **Crash safety:** if step 2's RPC succeeds but the test process dies before step 3, the marker survives. The next invocation reads it, picks `temp` as current, and rolls forward.
- **Verification:** Linux + `PCLOUD_LIVE_E2E=1` + `PCLOUD_LIVE_E2E_DESTRUCTIVE=1` + credentials; assert pre/post balance preserves the original password.
- **Acceptance:** test passes under the destructive gate, marker file is absent after success, simulated mid-test crash recovers cleanly.

## Item D3 — Row 142 `CryptoAccountTeamShare` IPC variant

- **Files:** `crates/pcloud-ipc/src/methods.rs` (new variant), `crates/pcloud-daemon/src/dispatch.rs` (new arm), `crates/pcloud-backends/src/shares_backend.rs` (backend method), `crates/pcloud-cli/src/commands.rs` (optional CLI surface), `C_FEATURE_PARITY_MATRIX.csv` + `STATUS.md` (flip Partial → Implemented).
- **Fix:** mirror the `CryptoShareFolder` pattern landed in fire 15 (P3.2) but for **team-share**: replace `mail` with `team_id`, keep the temppass + permission_bits + hint fields. Wire dispatch + backend.
- **Verification:** `cargo test -p pcloud-ipc proptest_methods_roundtrip`; new live verb-reached test `live_crypto_account_teamshare_dispatches_verb_reached` (the user's accounts are personal, not business — so verb-reached is the achievable proof).
- **Acceptance:** row 142 flips Partial → Implemented; `STATUS.md` headline 153/3 → 154/2.

## Item D4 — `notify-debouncer-full` swap (P4.3 follow-up)

- **Files:** `crates/pcloud-fs/Cargo.toml` (add dep), `crates/pcloud-fs/src/fs_watcher.rs` (replace hand-rolled debouncer + max-age guard).
- **Fix:** add `notify-debouncer-full` workspace dep, replace the custom `RecommendedWatcher` + `debounce_loop` setup with `notify_debouncer_full::Debouncer<RecommendedWatcher, FileIdMap>`. Preserve the `FsWatcherEvent` shape on the public surface so callers don't need to change.
- **Risk:** workspace `vendor/notify-dfly-fix` patches the `notify` crate; `notify-debouncer-full` depends on `notify`. The patched `notify` may or may not satisfy the debouncer's version constraint.
- **Decomposition:**
  - **D4.1** Add the dep, update `cargo check`. If the version pin fights the patch, document the obstacle in `DEFERRED-PROGRESS.md` and revert. Otherwise proceed.
  - **D4.2** Replace the watcher setup; preserve all 16 existing unit tests in `fs_watcher.rs`.
  - **D4.3** Remove the now-unused max-age guard added in fire 20; drop `PendingEntry` if `notify-debouncer-full` covers the case.
- **Verification:** `cargo test -p pcloud-fs --lib fs_watcher` — must reach 16/16 passing.
- **Acceptance:** zero hand-rolled debounce code in `fs_watcher.rs`; debouncer-full owns the cadence.

## Item D5 — Per-backend `ResilientTransport` migration (P4.1 follow-up)

- **Files:** `crates/pcloud-daemon/src/runtime.rs` (transport composition), `crates/pcloud-proto/src/builder.rs` (HTTP factory), `crates/pcloud-resilience/src/transport.rs`, plus per-backend wiring in `crates/pcloud-backends/src/{auth_backend,transfer_backend,public_link_backend,shares_backend,sync_backend,backup_backend,account_backend}.rs`.
- **Fix:** wrap each production `BinaryApiTransport` in `ResilientTransport` so circuit-breaker / retry-budget / token-bucket are reachable on every API call site. The factory is already in place per fire 18; this is the per-backend application of it.
- **Decomposition:**
  - **D5.1** `auth_backend` — wraps the login / userinfo / TFA paths. Smallest blast radius; canary backend.
  - **D5.2** `transfer_backend` — uploads + downloads.
  - **D5.3** `public_link_backend`.
  - **D5.4** `shares_backend`.
  - **D5.5** `sync_backend`.
  - **D5.6** `backup_backend`.
  - **D5.7** `account_backend`.
- **Verification:** existing per-backend unit tests; new integration test that observes the circuit-breaker opening on a forced-503 mock.
- **Acceptance:** every API call site goes through `ResilientTransport`; no raw `reqwest::Client::get` in production paths.

## Item D6 — RSA-OAEP wire-shape unification (P3.3 follow-up)

- **Files:** `crates/pcloud-crypto/src/share_temppass.rs:343-345` (currently returns `RsaBackendRequired`); `crates/pcloud-crypto/src/share_rsa.rs::wrap_share_invitation_b64`; `crates/pcloud-daemon/src/dispatch.rs` (multi-RPC orchestration); `crates/pcloud-backends/src/shares_backend.rs`.
- **Fix:** unify the `derive_temppass_wire` byte-shape with `wrap_share_invitation_b64` by orchestrating the multi-RPC flow at the daemon level. This requires the daemon to call `crypto_share_metadata` to fetch the recipient's RSA public key, then route through `wrap_share_invitation_b64` for the PclsyncCompat path. Replace the `RsaBackendRequired` early-return with the actual flow.
- **Verification:** existing `crates/pcloud-backends/tests/crypto_share_rsa_e2e.rs`; new daemon-level integration test exercising the full flow with mock RPC responses.
- **Acceptance:** rows 124 and 142 (live two-account E2E captured by item D3) flip from Partial → Implemented; `derive_temppass_wire` no longer surfaces `RsaBackendRequired`.

---

## Out-of-scope for this loop

These items genuinely cannot be closed without external action (the
companion "Non-AI dependencies" enumeration asks the user which they
can provide):

| Item | Blocked on |
|---|---|
| Live macOS / Windows / FreeBSD mount verification | SSH access to a host with the required FUSE driver pre-installed |
| C-client KAT capture | Access to a host running the official pCloud `pcloudcc` binary |
| Apple Developer notarisation | Apple Developer account |
| Authenticode EV signing | EV hardware token |
| TFA-enabled fixture account live tests | A pCloud account with TFA enabled (separate from accounts A/B) |
| Email-OTP injection for full `change_crypto_pass` | Either a captured OTP fixture or an SMTP mock |
| Human reviewer sign-off (`bd-1du.10`) | Non-AI |

The loop will explicitly skip these rather than thrash. They will be
re-classified in `DEFERRED-PROGRESS.md` as `[OUT-OF-SCOPE-PENDING-USER-RESOURCE]`
once the loop's own work is exhausted.

---

## Operating model

Each cron fire:

1. Reads `CLAUDEREV/DEFERRED-PROGRESS.md` to find the next unfinished item.
2. Picks one item; if scope > 30 min agent budget, decompose into a sub-step.
3. Executes the fix.
4. Verifies via `cargo check --workspace --all-targets` + `cargo fmt --all --check` + `cargo deny check` plus the per-item acceptance commands.
5. Updates `CLAUDEREV/DEFERRED-PROGRESS.md` with item ID, files touched, verification commands run, observed output.
6. If everything in this plan is done (D1–D6 closed and OOS items acknowledged), call `CronList` → `CronDelete`, write `CLAUDEREV/DEFERRED-COMPLETE.md`, stop.

Verification baseline (must hold across every fire):

- `cargo check --workspace --all-targets` exit 0
- `cargo fmt --all --check` exit 0
- `cargo deny check` clean
- `cargo doc --workspace --no-deps` warning count monotonically non-increasing (current floor: 41)

If a fire would break the baseline, **the fire reverts its own changes**
and logs the regression for analysis on the next fire.

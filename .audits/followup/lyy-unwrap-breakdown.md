# pcloud-rs-lyy — unwrap/expect sweep breakdown

## Baseline (2026-04-19)

- Raw `.unwrap()` / `.expect(` in `crates/*/src/`: **3013 sites**.
- Production-only (excluding `#[cfg(*test*)]`, `///`, `//!`): **138 sites** / 18 crates.
- ~2875 raw hits are in test modules / doctest examples; out of scope for this epic.

## Per-crate bucket (production-only)

| Crate | Count | Category |
|---|---|---|
| pcloud-crypto | 35 | SAFE — HMAC any-key, AES-32-byte, PBKDF2 infallible; already INVARIANT-annotated |
| pcloud-fs | 29 | SAFE — inode mutex, CString literals, `try_into` on `[u8;12]` |
| pcloud-daemon | 17 | MIXED — thread spawn, writer slot, cond-var |
| pcloud-sdk | 15 | SAFE — private mutex in `UploadSession` |
| pcloud-backends | 11 | SAFE — mock recorder, normalised-path split, just-inserted |
| pcloud-proto | 4 | HOT — **converted this session** |
| pcloud-idp/config | 7 | SAFE — internal mutexes |
| pcloud-plugin-api/compat/auth | 9 | SAFE — manifest serialize, fixed-header decode |
| pcloud-resilience/web | 4 | SAFE — manual clock, getrandom at MVP startup |
| pcloud-observability/mockserver | 4 | SAFE — exporter socket, canned JSON |
| store/fleet/plugin-backup-schedule | 3 | SAFE |

## This session

- 2 conversions: `http_download.rs:455` (retry_after guard), `:370` (write!(String)).
- 13 SAFETY annotations: inode.rs (module+2), write_journal.rs:293-299,
  integrity_sweeper.rs:392/409, macos.rs:2070, rpc_codec.rs:214-215,
  shm_producer.rs:249, exporter.rs:213, routes.rs:636, upload_session.rs
  (module), backends/mock.rs (module).
- Net: 138 → ~136; ~100 already carry prior INVARIANT/SAFETY.

## Remaining categories

- A: bare `.expect("… poisoned")` — ~40 — migrate to `LockExt::lock_or_poisoned`.
- B: fixed-length `try_into` — ~8 crypto/compat/fs — annotate only.
- C: HMAC/AES key-length — ~20 crypto — already annotated.
- D: OS-randomness startup — ~6 — typed startup errors.
- E: thread-spawn startup — ~5 — typed startup errors.
- F: prod-compiled test mocks — ~15 — covered by module SAFETY docs.

## Recommended wave order

1. Wave 2: migrate bare poisoned-mutex panics to `lock_or_poisoned` (~15 sites).
2. Wave 3: introduce `StartupError` in pcloud-daemon; thread through spawn sites.
3. Wave 4: convert OS-randomness startup panics to typed errors (pcloud-web, crypto::keys::Default).
4. Wave 5: audit-only — verify all existing SAFETY/INVARIANT comments are still accurate.

## Loop-3 batch A (pcloud-daemon + pcloud-ipc)
- Starting sites: 8 (prod, test-aware awk filter)
- Cat A (SAFETY): 5 fixed — `transport_factory.rs:162`, `transfer_bridge.rs:258`, `integrity_sweeper_service.rs:815/1082` (already had INVARIANT; left as-is with existing rationale), `mount_runtime.rs:847/887` (re-phrased as SAFETY). Thread-spawn Cat A sites in `integrity_sweeper_service.rs` and `audit_verifier_service.rs` retained with their existing INVARIANT comments pending Wave 3 (`StartupError` threading).
- Cat B (typed Err): 0 fixed.
- Cat C (LockExt): 0 — the single `.expect` on a condvar (`audit_verifier_service.rs:590`) was converted to inline log+recover, since `LockExt` targets `Mutex::lock`, not `Condvar::wait_timeout`.
- Cat D (debug_assert): 1 fixed — `audit_verifier_service.rs:590` condvar poisoning replaced with `log::error!` + `poisoned.into_inner()` silent recovery.
- Deferred cross-crate: 0.
- Ending sites: 7 (the 5 Cat A sites carry SAFETY/INVARIANT rationale; the 2 thread-spawn Cat A sites remain pending Wave 3).
- cargo check: clean. cargo test --lib: 228 passed, 0 failed (pcloud-daemon 209 + pcloud-ipc 19).
- Scope note: `pcloud-ipc` had 0 production `.unwrap()`/`.expect()` sites.

## Loop-3 batch D (remaining crates: cli, sdk, web, resilience, fleet, idp, config, engine, compat, secret, cache, session, mockserver)
- Starting sites: 7 (prod only, with balanced-paren `#[cfg(...test...)]` filter — the naive grep reports 580 hits but all except these 7 live in `#[cfg(all(test,...))]` or similar gated test modules). Crate breakdown: pcloud-web=2, pcloud-compat=3, pcloud-mockserver=2; cli/sdk/resilience/fleet/idp/config/engine/secret/cache/session reported 0 prod sites once test modules were excluded.
- Cat A (SAFETY): 3 fixed with new annotations — `pcloud-web/src/lib.rs:159` (generate_web_token_or_panic docs-backed RNG panic), `pcloud-mockserver/src/lib.rs:512` (hermetic mock state lock), `pcloud-mockserver/src/lib.rs:782` (canned serde_json Value to Vec<u8> infallibility).
- Cat A (already annotated, no edit): 4 — `pcloud-web/src/routes.rs:638` (CSRF mint RNG), `pcloud-compat/src/rpc_codec.rs:219/220` (length-checked `try_into` to fixed array), `pcloud-compat/src/shm_producer.rs:253` (post-sentinel-check NonNull on shmat return).
- Cat B/C/D: 0.
- Deferred: 0.
- Ending sites: 7 (all now carry SAFETY rationale; none remaining as bare `.expect`).
- cargo check --workspace: clean (one pre-existing unused-variable warning in pcloud-sdk unrelated to this batch).
- cargo test -p pcloud-web -p pcloud-mockserver -p pcloud-compat --lib: 20 passed (pcloud-web 11 + pcloud-mockserver 9), 0 failed; pcloud-compat has no lib tests.
- Per-crate breakdown: pcloud-web 2→2 annotated; pcloud-compat 3→3 annotated; pcloud-mockserver 2→2 annotated; all other assigned crates 0 prod sites.

## Loop-3 batch C (pcloud-backends + pcloud-proto)
- Starting production sites in scope: 11 (mock.rs x7, path_resolver.rs x1, upload_sessions.rs x1, transport.rs x2). All other hits in `pcloud-backends/src/` and `pcloud-proto/src/` live inside `#[cfg(test)] mod tests { ... }` and were skipped.
- Cat A/B/C/D/deferred: 4 / 0 / 7 / 0 / 0.
- Ending production sites: 4 (all Cat-A, SAFETY-annotated).
- `cargo check -p pcloud-backends -p pcloud-proto`: clean.
- `cargo test -p pcloud-backends -p pcloud-proto --lib`: 369 passed (170 backends + 199 proto), 0 failed.
- Notes:
  - `pcloud-backends::mock` now uses `LockExt::lock_or_poisoned` uniformly (dep was already present); per-callsite context labels encode `pcloud-backends::mock::<Type>::<method>`.
  - `pcloud-proto::transport` sites kept as Cat-A SAFETY-annotated `expect` because `pcloud-proto` does not depend on `pcloud-observability`; adding that dep is out of scope for this loop (would flip the crate's dependency-graph posture). Annotations document that the write-side critical section performs only infallible field assignments, so poisoning is unreachable on the read path.
  - `path_resolver::split_parent` and `UploadSessions::create` already had INVARIANT comments; upgraded to explicit SAFETY phrasing pointing at the specific normalisation / same-thread-insert invariant.

## Loop-3 batch B (pcloud-crypto + pcloud-fs + pcloud-observability)
- Starting sites: 51 (precise brace-tracking filter — pcloud-crypto=31, pcloud-fs=19, pcloud-observability=1; the raw grep reports 820 hits but the remainder live inside `#[cfg(test)] mod tests { ... }` blocks).
- Cat A (SAFETY): 13 — 10 new annotations added (`pclsync_auth_tree.rs:172/180/193/254`, `pclsync_sector.rs:161/371/443`, `pclsync_rsa.rs:322-323`, `pclsync_compat_profile.rs:157/188/213`, `lib.rs:2258/2286/2296`); the other in-scope sites already carried INVARIANT / `# Panics`-docs / inline-cite rationale and were left as-is.
- Cat B (typed Err): 0.
- Cat C (LockExt): 6 — all 6 `InodeTable` mutex `.expect("inode table mutex must not be poisoned")` call sites in `pcloud-fs/src/inode.rs` converted to `LockExt::lock_or_poisoned("inode::<method>")` (added `use pcloud_observability::LockExt;`; Cargo dep was already present). This eliminates the 6 raw `.expect()` sites.
- Cat D (debug_assert): 0.
- Deferred cross-crate: 0.
- Ending sites: 45 (crypto=31, fs=13, observability=1). Net reduction: 6 via Cat C; the 10 Cat-A annotation additions carry rationale without removing the `.expect()` itself (by design — infallible-by-invariant sites retain the panic as a defense-in-depth tripwire).
- `cargo check -p pcloud-crypto -p pcloud-fs -p pcloud-observability`: clean (pre-existing `function_casts_as_integer` warning on `pcloud-fs/src/platform/linux.rs:729` unrelated).
- `cargo test -p pcloud-crypto -p pcloud-fs -p pcloud-observability --lib`: 406 passed (crypto 174 + fs 194 + observability 38), 0 failed, 1 ignored (fs).
- Note: the two `let _ = lock` lines in `pcloud-observability/src/lock_ext.rs:192,205` have a pre-existing `#[allow(clippy::let_underscore_lock)]`-adjacent deny and were not touched per partition instruction.

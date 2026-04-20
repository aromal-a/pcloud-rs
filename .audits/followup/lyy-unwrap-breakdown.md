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

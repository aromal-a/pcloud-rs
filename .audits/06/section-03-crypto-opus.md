# Section 3 — Crypto Subsystem — Audit 06 (Opus)

Scope: `crates/pcloud-crypto/` re-audit post audit-05. Verifies
`sectors_sealed` persistence, SeqCst lockout, `EmptySector` reject,
BackendMismatch reachability, nonce budget discipline, constant-time
compares, zeroize discipline. Word budget <900.

## Summary

Audit-05 mid-tier findings are measurably fixed: `sectors_sealed` is now
persisted via `atomic_u64_serde` (`lib.rs:686`), the lockout pair is
SeqCst-ordered (`lib.rs:1401,1434-1436,1444-1447,1475,1506-1508`),
`SectorError::EmptySector` is explicit (`pclsync_sector.rs:151,329,357`)
and surfaces as `CryptoError::EmptySector` (`lib.rs:369,2653`). KAT
placeholders, compat-profile `Debug`, and `SymKeyVer1` `Clone` are
addressed. The pbkdf2-HMAC-SHA512, RSA-4096+OAEP, CBC-CTS-CS3, 128-ary
Merkle and reversible filename layer remain spec-faithful.

**One CRITICAL from audit-05 is NOT fixed**: `CryptoError::BackendMismatch`
is still unreachable in production. The test at
`tests/pclsync_compat_roundtrip.rs:251` still accepts it *or*
`NotYetWired`, which hides the gap.

## CRITICAL

**C-1. `BackendMismatch` remains declared-but-unreachable.**
`lib.rs:306` defines the variant. Zero production sites construct it
(`grep -n "CryptoError::BackendMismatch"` — only matches are docs, prior
audits, and the permissive test). Cross-backend dispatch still bails
with `NotYetWired` (`lib.rs:2014,2192,2214,2246,2282`) or
`MissingFileId` (`lib.rs:2495`) or `Locked` (`lib.rs:2485`). Impact:
callers cannot distinguish "feature pending" from "profile sealed under
a different backend", and the documented silent-corruption gate from
`docs/CRYPTO-BACKEND-PLAN.md:93` is not enforced. **Audit-05 H-1 was
acknowledged in the handoff but not implemented.** Fix: raise
`BackendMismatch { expected: self.effective_backend(), provided: … }`
from (at least) `change_password_with_context` and the Enhanced-only
sector/filename/metadata entry points before they fall through to
`NotYetWired`; tighten the roundtrip test to expect exactly
`BackendMismatch`.

## HIGH

**H-1. Nonce-budget counter uses `Relaxed` while the serde shim also
uses `Relaxed`.** `lib.rs:2502-2518` reads and increments
`sectors_sealed` under `Ordering::Relaxed`. Under concurrent `seal_sector`
calls on the same shell two threads can both observe `pre < budget_cap`
and proceed; the `fetch_add` is atomic but the gate is not a CAS. With
a 64-bit counter and a 2^32 cap this is extremely unlikely to matter in
practice, but the comment at `lib.rs:2497-2501` claims the guard
"refuses further seals" — a CAS-loop (compare_exchange_weak) or SeqCst
paired with a fetch_add would make the claim true. The serde shim at
`lib.rs:791-797,808-814` also snapshots under Relaxed, which means a
snapshot concurrent with fetch_add may persist a value slightly behind
reality; harmless for monotonic counters but worth `Acquire` on the
load for auditor clarity.

**H-2. Sentinel backend inference still unchanged from audit-05 H-2.**
`lib.rs:756-767` — `effective_backend` consults `self.backend` then
`keys.setup_fingerprint.is_some()`. It does **not** also consult
`self.pclsync_compat.is_some()`. A profile with both
`setup_fingerprint = Some` *and* `pclsync_compat = Some` (possible via
migration bug or tampered on-disk profile) will be inferred Enhanced and
silently use the wrong code path. Add a coherence check: if
`pclsync_compat.is_some()` the effective backend MUST be PclsyncCompat;
a populated `setup_fingerprint` in that case is grounds for refusing to
load the profile (return a new `CryptoError::ProfileInconsistent`).

## MEDIUM

**M-1. Plaintext return from `open_sector` still un-zeroized.**
`pclsync_sector.rs:498` — `plaintext.to_vec()` clones a `Zeroizing<Vec>`
into a plain `Vec`, so the caller copy is not zeroed on drop. L-4 from
audit-05 recorded the TODO; still present (file has
`pcloud-rs-8mb.28/L-4` TODO at line 495). Upgrade the public return to
`Zeroizing<Vec<u8>>` to end the discipline leak.

**M-2. Brute-force lockout timestamp uses wall-clock
`SystemTime::now()`.** `lib.rs:876-881` — if the system clock rewinds
(NTP step, VM migration, operator error) `now - last` underflows to
`saturating_sub == 0`, which forces backoff to expire immediately on
the *next* failure wave. The SeqCst ordering is correct; the clock
source is not. Use `Instant` for the window check and persist only a
failure *count*, OR detect `now < last` and treat it as "still inside
the window" conservatively.

**M-3. Nonce-budget counter zeroization on key rotation is undocumented
in-code.** `lib.rs:684-685` tells the daemon it is responsible; no
code currently resets `sectors_sealed` on `change_password` /
`setup`. Grep: `sectors_sealed` is only written at the `fetch_add` in
`seal_sector`. A password rotation today leaves the stale counter; if
the rotation effectively renewed the per-file keys, the budget counter
should be zeroed or the gate is permanently tripped. Either reset
explicitly inside `change_password_with_context` or document the bead.

## LOW

**L-1.** `lib.rs:892` — `failures.min(40)` silently clamps very large
counters; harmless given the 30-minute cap, but a `debug_assert!` that
`failures <= MAX_CONSECUTIVE_FAILURES + margin` would make the invariant
auditable.

**L-2.** `pclsync_sector.rs:495-497` — TODO(L-4) is dangling since
audit-05; either promote to a bead or land the type change.

**L-3.** `atomic_u32_serde` / `atomic_u64_serde` (`lib.rs:787-815`) have
no round-trip tests; one `#[test]` that serializes a shell with
`consecutive_failures = 7` and `sectors_sealed = 12345`, re-deserializes,
and asserts both values survive would prevent a silent regression of the
persistence claim.

**L-4.** `pclsync_compat_profile::Debug` (manual impl, audit-05 L-2
fix) should have a unit test asserting the password / private-key
fields are *not* in the `Debug` output. Grep shows none.

## Strengths confirmed

- Constant-time compares audited end-to-end (`subtle::ConstantTimeEq` in
  `lib.rs:1852,1915`, `pclsync_sector.rs:488`, `keys.rs:225`,
  `pclsync_auth_tree.rs:291,298,321,333`, `pclsync_filename.rs:377`,
  `share_temppass.rs:230`, `pclsync_compat_profile.rs:288`,
  `pclsync_rsa.rs:226-230`).
- Zeroize discipline: `SecretBytes`/`SecretString` wrap all long-lived
  secrets; `pclsync_kdf.rs`, `pclsync_rsa.rs::SymKeyVer1`, sector
  plaintext buffers use `ZeroizeOnDrop` / `Zeroizing`.
- Lockout SeqCst pairing is correct; audit-05 M-4 closed.
- `EmptySector` reject is tested (`pclsync_sector.rs:547-567`).
- PBKDF2-HMAC-SHA512, RSA-4096+OAEP, AES-CTR LE, CBC-CTS-CS3,
  128-ary Merkle primitives match the documented wire layout.

## Verdict

Audit-06 net result: audit-05 C-1, C-2, M-4, L-2 closed; H-1
(BackendMismatch) regressed back to CRITICAL because the permissive
test masks the gap; H-2 (sentinel) still open; several new
observations around nonce-budget ordering and clock source.

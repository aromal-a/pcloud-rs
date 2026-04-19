# Audit 06 — Section 3: Crypto Subsystem
**Auditor:** Sonnet 4.6 (independent cross-validator)
**Date:** 2026-04-18
**Scope:** `crates/pcloud-crypto/src/` and `crates/pcloud-crypto/tests/`
**Baseline:** post audit-05 (sectors_sealed persisted, SeqCst lockout, EmptySector reject, live KAT passed, offline KAT in CI)

---

## Findings

### HIGH

**H-1: `sectors_sealed` budget check uses `Relaxed` ordering — potential TOCTOU in concurrent callers**
`lib.rs:2503–2518`. The pre-seal load (`Ordering::Relaxed`) and the post-seal `fetch_add` (`Ordering::Relaxed`) are both relaxed. If two threads call `seal_sector` simultaneously on the same `CryptoShell` and the counter sits just below `budget_cap`, both can read the pre-check value, both pass, and the counter overshoots by one. The `SeqCst` discipline applied to the lockout counters (`lib.rs:1401,1434`) is conspicuously absent here. In practice `CryptoShell` is not designed to be shared across threads without external locking (it is `!Sync` by intent), but the `AtomicU64` type signals concurrency-safety to callers and the discrepancy creates a latent correctness gap. Remediation: either add a `// SAFETY:` exclusion comment proving `CryptoShell` cannot be shared, or use `AcqRel`/`SeqCst` on the budget pair to match the lockout counter discipline.

**H-2: `cache_ttl_secs` field is dead policy — key material has no automatic eviction**
`keys.rs:57–72`. The field is serialised (`pub cache_ttl_secs: u64`) and defaults to 300 s, but the daemon never starts a timer keyed on this value (documented as `TODO` in the field comment). Until the auto-stop timer is wired, an unlocked shell holds the Argon2id master key in memory indefinitely after the last authenticated operation. The dead field creates a false sense of security for operators who set it in a profile. Remediation: wire the tokio timer in the daemon runtime on every successful `start()`, or gate the field behind `#[cfg(feature = "ttl-enforcement")]` with a compile-time note that it has no effect until the feature is wired.

---

### MEDIUM

**M-1: `temppass` signature uses HMAC-SHA256 (symmetric) instead of RSA — not interoperable with C invitee path**
`share_temppass.rs:39–45, 211–222`. The module explicitly documents that `TemppassBlob::sign` uses the active master key as the HMAC key rather than the RSA-4096 user private key used by the C client (`prsa_sign_sha256_hash`). This means a Rust-generated temppass blob cannot be accepted by an invitee using the official pCloud app or C client: the invitee side expects an RSA signature. The documentation correctly calls this out as pending `bd-1du.5`, but the implementation is in production-adjacent state (the functions are exported, the tests pass, the IPC surface is wired). Any user who shares a crypto folder to a non-Rust invitee will produce an unacceptable blob. Remediation: gate `derive_temppass_wire` behind a runtime `CryptoBackend::Enhanced` check that returns `CryptoError::NotYetWired` when the profile is `PclsyncCompat`; or surface a clear user-facing error before the share call completes.

**M-2: `sectors_sealed` counter is not reset on key rotation — budget carries over**
`lib.rs:680–686`. The field doc says "On key rotation (password change / setup) the daemon is responsible for zeroing this counter so the new key gets a fresh budget." Neither `change_password_unlocked` (`lib.rs:1616`) nor `change_password_pclsync_compat_reencoded` (`lib.rs:1902`) calls `self.sectors_sealed.store(0, …)`. The responsibility is fully delegated to the daemon caller with no enforcement in the shell. A daemon that rotates the key but forgets to reset the counter immediately re-trips `NonceBudgetExhausted` if the counter was near the cap. Remediation: reset `sectors_sealed` to 0 inside `change_password_unlocked` and the PclsyncCompat equivalent as a defensive invariant, then document that the daemon need not do so.

**M-3: Offline KAT does not exercise the actual sector decrypt path — only blob parsing**
`tests/pclsync_compat_kat_offline.rs:1–24`. The offline KAT verifies fixture SHA-256 digests, parses `priv_key_ver1` / `pub_key_ver1` blobs, and confirms RSA-4096 modulus length. It does **not** decrypt the committed `kat-ciphertext-v1.bin` against `kat-plaintext-v1.bin`. That round-trip is gated behind `#[ignore]` in `pclsync_compat_kat_live.rs` and requires `PCLOUD_KAT_PASSWORD`. CI therefore never exercises sector decrypt correctness offline. Remediation: commit a test-only synthetic keypair with a known password and add an offline test that calls `open_sector` against the committed ciphertext fixture with that key; gate only the live key-unwrap variant behind `#[ignore]`.

**M-4: `EmptySector` reject is Enhanced-path-only — PclsyncCompat path lacks the guard**
`lib.rs:363–370`. The `EmptySector` error variant and its docstring were added in audit-05. Inspection of `seal_sector_with_context` shows the empty-plaintext check (`if plaintext.is_empty()`) lives in the function body — verify that the PclsyncCompat branch (`pclsync_sector::seal_sector`) also rejects empty input. If `pclsync_sector.rs` passes a zero-length plaintext through its "short path" (`datalen < 16` branch, `pclsync_sector.rs:PCLSYNC_SECTOR_SIZE`), an empty sector would produce a valid-looking frame with no plaintext bits — the same vulnerability that the Enhanced guard was meant to close. Remediation: add an explicit `if plaintext.is_empty() { return Err(SectorError::EmptyPlaintext) }` at the top of `pclsync_sector::seal_sector`.

---

### LOW

**L-1: `Unlocking` state is observable for a non-zero duration — narrow window but unnecessary**
`state.rs:43–46, lib.rs:1419`. The `UnlockState::Unlocking` variant is set before `derive_key_material` is called and is cleared (to `Unlocked` or `Locked`) after the fingerprint check. Argon2id derivation takes ~100–300 ms at default parameters. During this window any IPC caller that reads `is_started()` sees `false`, which is correct, but a caller that directly inspects `unlock_state` (which is `pub`) sees `Unlocking`. No current IPC path exposes `Unlocking` to external peers, but the pub field creates a future risk. Remediation: either mark `unlock_state` `pub(crate)` or add a doc comment explicitly banning external branching on `Unlocking`.

**L-2: `SetupFingerprint` is `Debug`-derived and prints raw bytes — no redaction**
`keys.rs:45–46`. `SetupFingerprint(pub [u8; 32])` derives `Debug` without redaction. It is documented as non-secret, and per the design it reveals no key bits (it is an HMAC of the derived key, not the key itself). However, in a log line an attacker with the fingerprint can run an offline Argon2id guessing attack. Remediation: implement a custom `Debug` that prints only the first 4 bytes and elides the rest (e.g. `SetupFingerprint(a1b2c3d4…)`), consistent with the crate's policy of minimising what appears in logs.

**L-3: `hint` field (`Option<String>`) is printed verbatim in `CryptoShell` `Debug`**
`lib.rs:818–832`. The `Debug` impl for `CryptoShell` includes `.field("hint", &self.hint)`. If a user sets a hint that contains partial password material (a common pattern with weak hint policies), that appears in any debug log. Remediation: print `hint` as `"Some(<redacted>)"` / `"None"` or truncate to 4 characters.

**L-4: Dual-backend wire-incompatibility is not surfaced at `setup()` time for `PclsyncCompat`**
`lib.rs:152–193, 756–767`. A user who provisions a new profile gets `PclsyncCompat` by default, but if `pclsync-v2` feature is not enabled the effective backend falls back silently (the `effective_backend()` inference path). No error or warning is emitted at `setup()` time to tell the operator that their profile will not interoperate with the official pCloud apps if the feature flag is absent. Remediation: emit a `tracing::warn!` in `setup()` when the resolved backend differs from the explicit `backend` field (or when `backend` is `None` and the feature flag state changes the resolved backend).

---

## Dual-Backend Assessment

The dual-backend architecture (`CryptoBackend::PclsyncCompat` gated on `pclsync-v2` feature / `CryptoBackend::Enhanced` always available) is structurally sound:

- Backend mismatch at sector ops is caught by `CryptoError::BackendMismatch` (`lib.rs:305–311`).
- `MissingFileId` (`lib.rs:317–329`) prevents PclsyncCompat sector ops from silently using the Enhanced derive path.
- `effective_backend()` migration inference (`lib.rs:756–767`) is conservative.
- `pclsync_compat_state` (RSA priv key + sym-key caches) is `#[serde(skip)]` — not persisted — and is cleared on `stop()`.
- The `SealedSectorFrame.auth_tag` detached-tag field correctly disambiguates the two wire layouts.

Remaining gap: the PclsyncCompat path's folder-key and file-key caches are populated lazily by daemon calls to `crypto_getfolderkey` / `crypto_getfilekey`, but no eviction policy exists for stale cache entries when a folder's key is rotated server-side. This is an audit-05 carry-over not resolved by audit-06.

---

## Summary Table

| ID  | Severity | Title                                                       |
|-----|----------|-------------------------------------------------------------|
| H-1 | HIGH     | `sectors_sealed` Relaxed ordering — TOCTOU potential        |
| H-2 | HIGH     | `cache_ttl_secs` dead — no automatic key eviction           |
| M-1 | MEDIUM   | Temppass HMAC not RSA — PclsyncCompat invitee incompatible  |
| M-2 | MEDIUM   | `sectors_sealed` not reset on key rotation                  |
| M-3 | MEDIUM   | Offline KAT skips sector decrypt — CI blind to wrong-key    |
| M-4 | MEDIUM   | `EmptySector` guard absent in `pclsync_sector` path         |
| L-1 | LOW      | `Unlocking` state publicly observable                       |
| L-2 | LOW      | `SetupFingerprint` Debug prints full 32 bytes               |
| L-3 | LOW      | `hint` field unredacted in `CryptoShell` Debug              |
| L-4 | LOW      | Dual-backend mismatch not warned at `setup()` time          |

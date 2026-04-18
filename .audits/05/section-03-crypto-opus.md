# Section 3 — Crypto Subsystem — Audit 05 (Opus)

Scope: `crates/pcloud-crypto/` with focus on post-audit-04 pclsync-v2
dispatch landing. Word budget: <900.

## Summary

Wave-1 pclsync-compatible primitives are spec-faithful, zeroize-disciplined,
and constant-time where it matters. The Enhanced path is unchanged and
remains sound. Dispatch is correct on the happy paths, but there are real
gaps: `CryptoError::BackendMismatch` is defined but never raised, cross-backend
unlock is not truthfully reported, and the "live KAT" does not actually
assert what STATUS.md claims.

---

## CRITICAL

**C-1. Live KAT is a conditional skip masquerading as proof.**
`crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs:131-149` — test is
`#[ignore]` AND double-gated on `PCLOUD_KAT_PASSWORD`; absent the env var
the body `return`s with zero assertions. `STATUS.md:9,17-21` describes
"all confirmed" via this harness. Nothing runs in normal `cargo test`.
Remediation: commit an offline KAT (deterministic fixture produced by
linking `pcrypto.c`) that asserts byte-exact sector ciphertext + tag with
no env gate; keep the live test in addition.

**C-2. `kat_from_c`/`kat_from_c_source` placeholders ship empty.**
`pclsync_modes.rs:500-504` and `pclsync_sector.rs:714-718` are `#[ignore]`
stubs with a TODO pointing at `bd-1du.10`. The Wave-1 claim of byte-for-byte
C parity therefore rests on self-consistency only (the NIST CTR KAT at
`pclsync_modes.rs:367` covers standard CTR, not the pclsync-native
`aes256_ctr_pclsync_xor_inplace`). Until a C-generated vector lands in
`pclsync_sector`, any on-disk interop claim is unproven.

---

## HIGH

**H-1. `BackendMismatch` error variant is unreachable.**
`lib.rs:306` defines it; no code path in the crate constructs it
(`grep` confirms). Cross-backend attempts instead surface as
`NotYetWired` (`lib.rs:1993`) or `MissingFileId` (`lib.rs:2473,2575`).
Consumers cannot distinguish "feature pending" from "wrong backend"
which matters for IPC error UX and for the roundtrip test at
`tests/pclsync_compat_roundtrip.rs:251` that accepts *either*. Fix:
emit `BackendMismatch { expected, provided }` from
`change_password_with_context` (`lib.rs:1992`), from the Enhanced-legacy
`seal_sector`/`open_sector` when effective backend is PclsyncCompat
(`lib.rs:2458,2565`), and anywhere else dispatch bails.

**H-2. Sentinel-inferred backend silently migrates on first successful
unlock.** `lib.rs:1348-1368` — a historical profile with
`setup_fingerprint.is_some()` is assumed Enhanced, and `self.backend`
is stamped on successful `start()`. If a caller ever manages to seed a
PclsyncCompat profile into a shell whose legacy `setup_fingerprint`
was also populated (e.g. a buggy persistence migration or an attacker
with write access to the profile file), the sentinel will mis-classify
and the unlock path selected may mis-decrypt. The sentinel should also
check `self.pclsync_compat.is_some()` → always PclsyncCompat.
`effective_backend` at `lib.rs:748-759` does not combine the two
signals.

**H-3. Pclsync-native CTR is documented-little-endian, not
wire-compatible with the legacy C client on BE hosts.**
`pclsync_modes.rs:93-101,117` explicitly chooses LE counter bytes. The
C source XORs the counter as a native `unsigned long` store
(`pcrypto.c:148`). This is fine for x86_64 but the module docstring
still advertises "wire-compatible with the legacy C client". Either
rename to `_le` or gate the implementation on `cfg(target_endian)`
and refuse BE targets — otherwise a future arm64be build silently
corrupts priv-key-unwrap. At minimum add a `const _: () =
assert!(cfg!(target_endian = "little"))` compile-time guard.

---

## MEDIUM

**M-1. RSA-OAEP fallback parser opens a padding-oracle shape.**
`tests/pclsync_compat_kat_live.rs:80-128` cascades multiple
normalizations (left-pad, right-pad, 8-byte strip…) through
`oaep_unwrap`, logging each failure mode. This is test-only code, but
if this pattern migrates into production the per-normalization success
signal is observable. Flag `normalize_candidates` as
`#[cfg(test)]`-only; never reuse in daemon code.

**M-2. Short-plaintext sector path XORs plaintext into `rnd` with
datalen as the only gate.** `pclsync_sector.rs:364-380` is faithful to
`pcrypto.c:505-513`, but for `datalen == 0` the ciphertext is empty
while the auth tag still encrypts `rnd || hmac[0..16]`. `seal_sector`
accepts `plaintext.len() == 0` (tested in `seal_open_roundtrip_empty`).
The C client never calls encode with len=0 in practice. Either add a
`SectorError::EmptyPlaintext` for symmetry with
`PlaintextTooLong`, or document that empty sectors are an intentional
extension; right now it is silently supported.

**M-3. `pclsync_auth_tree` ships the pure-HMAC half only.**
`pclsync_auth_tree.rs:36-47` (DIVERGENCE NOTE) — parent tags omit the
AES-256-ECB wrap from `pcrypto_sign_sec` (`pcrypto.c:644-654`), so the
produced Merkle root is NOT byte-identical to C. This is already
disclosed but STATUS.md lists this as Wave-1 primitive complete; fix
the STATUS wording or land the AES wrap.

**M-4. Brute-force lockout counters are `Relaxed`-ordered atomics.**
`lib.rs:1391-1403,1417-1420,1453-1466,1483-1487` — under concurrent
unlock attempts the `fetch_add` + `store(now)` pair is not atomic, so a
racing pair of wrong-password attempts can leave `last_fail_at` lower
than expected and shorten the backoff. Use a `Mutex<LockoutState>` or
at minimum `AcqRel` with a CAS loop.

---

## LOW

**L-1.** `pclsync_kdf.rs:105` — `.expect("pbkdf2::<Hmac<Sha512>> is
infallible")` is correct but the salt length is not statically
enforced against `PCLSYNC_PBKDF2_SALT_LEN`; type signature uses
`&[u8; 64]` which does enforce it at the boundary — fine.

**L-2.** `pclsync_rsa.rs:169` — `SymKeyVer1` derives `ZeroizeOnDrop`
on the aggregate but `sym_type`/`flags` (`u32`) are not zeroize-
sensitive; harmless.

**L-3.** `pclsync_compat_profile.rs:246,274` — spacing `&kek.key,&...`
is non-conforming; cosmetic.

**L-4.** `pclsync_sector.rs:483` — `plaintext.to_vec()` after a
`Zeroizing<Vec<u8>>` copies the recovered bytes into an un-zeroized
`Vec`. The caller receives plaintext in a non-zeroizing container.
Consider `open_sector` returning `Zeroizing<Vec<u8>>` so the Enhanced
discipline extends to the pclsync path.

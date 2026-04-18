# Audit 05 — Section 1: C-to-Rust Feature Parity & API Coverage (Opus)

**Date:** 2026-04-18
**Scope:** `C_FEATURE_PARITY_MATRIX.csv`, `STATUS.md`, `REJECTED-RATIONALES-14042026.md`, dual-backend crypto, `pclsync_compat_kat_live.rs`, the 5 Partial rows.

## Headline verification

- Matrix row count: **186** (csv confirms).
- Status column tally (col 4): **Implemented 153 / Partial 5 / Missing 0 / Rejected 28**. STATUS.md headline (`STATUS.md:23`) matches, but the "Parity snapshot" table further down (`STATUS.md:493-496, 514`) still prints **155 / 3 / 0 / 28**. Inconsistency within the same doc.
- Rejected rationales: `REJECTED-RATIONALES-14042026.md` contains exactly **28** `^Row` headings — 1:1 with matrix.

Findings: **2 CRITICAL, 3 HIGH, 3 MEDIUM, 2 LOW.**

---

## CRITICAL

### C-01 — `STATUS.md` self-contradicts its own headline count
The file asserts `153/5/0/28` at `STATUS.md:23` but then the "Parity snapshot" table at `STATUS.md:493-496` and the summary table at `STATUS.md:514` still say `155/3/0/28`. A parity-truth document that disagrees with itself cannot be used as a release gate.
**Remediation:** rewrite both tables to match the audit-04 correction (`153/5/0/28`) or add an explicit "superseded" banner above the stale tables. File: `STATUS.md:493-520`.

### C-02 — Row 93 `upload_writefromfile` is not Partial, it is effectively Missing on the IPC/daemon path
`crates/pcloud-daemon/src/runtime.rs:2683-2714` is a hard stub returning `InternalError: "not yet wired: requires server-side copy via UploadWriteFromFileRequest (bd-1du)"`. The proto DTO (`crates/pcloud-proto/src/methods/upload.rs:264-322`) and the high-level helper (`crates/pcloud-proto/src/transfer_api.rs:497`) exist, but no caller reaches them from IPC: the only dispatch branch explicitly errors. The IPC variant at `crates/pcloud-ipc/src/methods.rs:1056` carries a local-file `path` + `offset`, not the C primitive's `(fileid, hash, offset, count)` — the wire contract is still wrong, so even the stub couldn't be "completed" without an IPC-schema change.
**Remediation:** redesign `Request::UploadWriteFromFile` to carry `{ upload_session_id, source_fileid, source_hash, offset, count }` and remove the `local_path` field; wire `TransferRuntime` to invoke `UploadWriteFromFileRequest`; keep matrix row 93 Partial until a live round-trip test exists. Reclassify to Missing if the IPC rename is not done in this audit cycle.

---

## HIGH

### H-01 — Row 26 `psync_tfa_has_devices` / Row 27 `psync_tfa_type`: no implementing code at all
Matrix cites `crates/pcloud-auth/src/orchestrator.rs` as the Rust reference but the notes themselves admit "workspace grep … confirms zero implementing code". A row whose Rust reference file does not contain the feature is not honestly "Partial" — it is "Missing with scaffolding in a plausible host file". Either add a `SessionManager::tfa_has_devices() / tfa_method()` accessor surfaced via IPC/CLI, or flip both rows to Rejected with a rationale (adaptive TFA UI out of scope).
**Remediation:** implement or reclassify before closing `bd-1du.10`.

### H-02 — Rows 146 & 182 `psync_crypto_share_folder` / `psync_crypto_account_teamshare` are symmetric-signature-only
Notes acknowledge "RSA-4096 path pending". `crates/pcloud-crypto/src/share_temppass.rs` produces an HMAC-SHA256 signature instead of the RSA-4096 signature C-clients verify. Result: **Rust-generated invitations are non-functional against legacy C recipients.** This is an interop break, not a minor gap. Gate closure on `bd-1du.5`.
**Remediation:** land RSA-4096 signing in `share_temppass.rs`; keep `bd-1du.10` blocked; add a live C-client cross-verification test.

### H-03 — KAT test `pclsync_compat_kat_live.rs` overstates coverage
STATUS.md line 25-27: "Wire-verified KAT now proves the PclsyncCompat primitives byte-decode ciphertext produced by pCloud's official web client." The test (`crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs`) is:
- `#[cfg(feature = "pclsync-v2")]` AND `#[ignore = "live KAT: requires $PCLOUD_KAT_PASSWORD + extracted fixtures"]` AND double-gated on `PCLOUD_KAT_PASSWORD` (`:131-144`). It does not run under `cargo test` or `cargo test --ignored` on CI unless the env var is set. There is no CI job that sets it (no `.github/workflows/*.yml` reference found).
- The long panic message at `:283-300` is a known-bad-fixture escape hatch: if `kat-file-sym-key-wrapped.bin` (504 B vs 512 B RSA-4096 block) cannot be OAEP-unwrapped under any of 3 heuristic normalizations, the test panics with a "regenerate fixtures" instruction. This is developer-only, not a release gate.
- Claim "byte-identical over 4096 B plaintext" (`:359-372`) is valid when the test passes — but only for one fixture / one password / sector id 0. It does not exercise multi-sector, file-hash-bound HMAC path (`pfscrypto.c:237-248`), or the Enhanced backend.
**Remediation:** (a) add a CI job that secrets-injects `PCLOUD_KAT_PASSWORD` and runs `--ignored`; (b) extend fixtures to ≥2 sectors and non-zero sector id; (c) soften the STATUS.md wording to match what the test actually covers; (d) fix the `scripts/extract-pclsync-kat.py` encoding heuristic so the 504-byte workaround is removed.

---

## MEDIUM

### M-01 — `CryptoBackend::default() == PclsyncCompat` without explicit opt-in warning
`crates/pcloud-crypto/src/lib.rs:172` defaults to PclsyncCompat. The Enhanced backend is the newer / stricter path. Document-truth rule requires that operators who want Enhanced explicitly opt in; doc files should say so. No such note was found in `CLAUDE.md` or `STATUS.md`. Add a runtime log line at backend selection and a section in operator docs.

### M-02 — `Request::UploadWriteFromFile` variant still shipped with a `path: String` field
`crates/pcloud-ipc/src/methods.rs:1056` + the proptest at `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:623-625` fuzz an IPC shape that will be redesigned (see C-02). Proptest coverage on a schema you know is wrong is false assurance.
**Remediation:** deprecate/rename the variant before the proptest lands as release evidence.

### M-03 — Matrix `rust_reference` column is a hint, not a proof
Rows 26, 27, 93, 146, 182 cite Rust files that do **not** contain the feature (confirmed by grep). Reviewers skim this column as "implementation exists here". Add a second column `impl_symbol` (function/type name) that must resolve with `rg` or fail CI.

---

## LOW

### L-01 — 28 Rejected rationales are 1-per-row but not cross-linked
`REJECTED-RATIONALES-14042026.md` uses `## Row N` headings; matrix notes do not hyperlink back. Add `(see REJECTED-RATIONALES-14042026.md#row-N)` to every Rejected note.

### L-02 — KAT README provenance incomplete
`crates/pcloud-crypto/tests/fixtures/pclsync_v2/README.md` exists (script generates it) but commit provenance — who uploaded the KAT plaintext, when, which account — is not git-logged as a signed note. For a reproducibility audit, add a signed `provenance.txt` next to the fixtures.

---

## Verdict
`bd-1du.10` must remain open. Genuine blockers: C-02, H-01, H-02, H-03 (CI gate). Once C-01 is cleaned and H-02 lands, headline count will drop to `155/3/0/28` honestly (Enhanced path flipping rows 146/182). Until then, "parity" claims in release docs are premature.

# Audit 06 §1 — C-to-Rust Feature Parity (Sonnet independent review)

**Auditor**: Sonnet 4.6 (independent, cross-validating Opus audit-05)
**Date**: 2026-04-18
**Post-audit basis**: audit-05; matrix 153/5/0/28; STATUS.md updated; row 93 IPC rewired (stub); offline KAT in CI

---

## Matrix Tally Cross-Check

CSV machine-count: **153 Implemented / 5 Partial / 0 Missing / 28 Rejected = 186 rows.**
Matches STATUS.md headline exactly. No discrepancy detected.

---

## CRITICAL

None identified in §1 parity scope. The prior CRITICAL (row 93 local-file shim with OOM vector) was correctly reverted post-audit-04/05. The daemon stub at `crates/pcloud-daemon/src/runtime.rs:2705-2716` now returns an explicit error string rather than silently executing wrong semantics.

---

## HIGH

### H-01 — Rows 26/27 (`psync_tfa_has_devices`, `psync_tfa_type`): zero implementing code, no resolution path declared

**Severity**: HIGH
**Files**: `crates/pcloud-auth/src/orchestrator.rs` (entire file — no `has_devices`/`tfa_type` surface present), `C_FEATURE_PARITY_MATRIX.csv` rows 23-24

Both rows are correctly marked `Partial` in the CSV following audit-04 §1-sonnet H-01/H-02. However, neither row carries a concrete remediation commitment: the CSV note says "Needs a SessionManager accessor + IPC + CLI, **or flip to Rejected if adaptive TFA UI is out of scope**" — but neither path has been taken. The matrix carries open Partial rows with no linked bead and no deadline. Per the audit rules: *"Anything `Partial` without a linked bead = HIGH."*

**Remediation**: Either (a) create `bd-1du.11` (or sub-bead) tracking TFA introspection surface and cite it in the CSV, or (b) conduct a scope decision and flip both rows to `Rejected` with documented rationale in `REJECTED-RATIONALES-14042026.md`. The current limbo state violates the parity honesty requirement.

---

### H-02 — Rows 124/142 (`psync_crypto_share_folder`, `psync_crypto_account_teamshare`): RSA-4096 gap blocks real C-client interop

**Severity**: HIGH
**Files**: `crates/pcloud-crypto/src/share_temppass.rs:39,213,278,315,369`, `C_FEATURE_PARITY_MATRIX.csv` rows 124/142

`share_temppass` correctly documents the symmetric-only limitation (`HMAC-SHA256` where C requires `RSA-4096-OAEP` signature). The `TemppassBlob::sign` call at `share_temppass.rs:213` is the single swap point. Bead `bd-1du.5` is referenced in the module doc and both CSV rows. **However**: Rust-generated share invitations are explicitly noted as non-functional for C-client recipients. This is a genuine interoperability regression for any deployment with mixed Rust/C clients — a realistic enterprise scenario. The parity claim on these rows is `Partial` which is correct, but the risk level is HIGH because it silently produces invalid share tokens without error surfacing to the end user.

**Remediation**: Ensure the public-link/share CLI surfaces a clear user-visible warning ("crypto share requires RSA-4096; not yet supported") rather than silently producing a non-functional invite. Track under `bd-1du.5`.

---

## MEDIUM

### M-01 — Row 93 (`upload_writefromfile`): IPC variant exists but daemon handler is a permanent stub

**Severity**: MEDIUM
**Files**: `crates/pcloud-daemon/src/runtime.rs:2693-2716`, `crates/pcloud-backends/src/transfer_backend.rs:603-610`, `crates/pcloud-ipc/src/methods.rs:1058`

The `Request::UploadWriteFromFile` IPC variant was correctly rewired to the right C-primitive shape (params: `upload_session_id`, `source_fileid`, `source_hash`, `offset`, `count`) and the daemon handler now returns an explicit stub error. The TODO at `transfer_backend.rs:603` describes the two-step remediation path. This is correctly `Partial`. The medium severity reflects that the stub error is not surfaced through a typed error code — `runtime.rs:2716` returns a raw string, not a structured `Response::Error` with a machine-readable code, making it harder for SDK consumers to distinguish "not implemented" from a transient network error.

**Remediation**: Return a structured `Response::Error { code: ErrorCode::NotImplemented, message: "..." }` from the stub. Add a proptest asserting the response is `NotImplemented`.

---

### M-02 — `psync_send_publink`: matrix says `Implemented`, CLAUDE.md previously said `Missing` — reconcile documentation

**Severity**: MEDIUM
**Files**: `C_FEATURE_PARITY_MATRIX.csv` row 42, `CLAUDE.md` (§Backup/device/account, "psync_send_publink remains missing"), `crates/pcloud-backends/src/public_link_backend.rs:954`, `crates/pcloud-daemon/src/runtime.rs:747`

`send_publink` is implemented end-to-end (proto → backend → daemon dispatch → SDK → CLI). The matrix row 42 correctly marks it `Implemented` with test citations. However, `CLAUDE.md` under "What Is Left To Do / Backup / device / account utility progress" still states *"`psync_send_publink` remains missing"*. This is a stale CLAUDE.md claim that contradicts the matrix. Per documentation discipline rules, CLAUDE.md must be updated to remove the false missing claim.

**Remediation**: Remove the stale `psync_send_publink` missing note from `CLAUDE.md` §Backup/device.

---

### M-03 — Row 149 (`ptree_public_link`): matrix flipped to `Implemented` but CLAUDE.md still calls it `Partial`

**Severity**: MEDIUM
**Files**: `C_FEATURE_PARITY_MATRIX.csv` row 149, `CLAUDE.md` §"What Is Left To Do / bd-1du.10"

The CSV row 149 is marked `Implemented` (path-based IPC wired via `PublicLinkPathResolver`; CLI command `CreateTreeLinkFromPaths` dispatches to `Request::CreateTreePublicLinkFromPaths`). CLAUDE.md §bd-1du.10 still lists "land a `Request::CreateTreePublicLinkFromPaths` IPC variant with server-side path resolution" as remaining work. The bead closure description in STATUS.md says "bd-1du row 149 closed." CLAUDE.md was not updated to reflect this closure.

**Remediation**: Update CLAUDE.md §bd-1du.10 to remove the row 149 item and note closure.

---

### M-04 — Chunked upload: resumability is implemented but idempotency on partial-write retry is not explicitly tested

**Severity**: MEDIUM
**Files**: `crates/pcloud-backends/src/transfer_backend.rs:649,984,1636`

`upload_bytes_chunked` drives the full state machine with `upload_resume_state` persistence. `upload_info` is called for sha1 verification before commit. However, the test suite (`chunked_upload_happy_path_drives_create_write_info_save`) covers the happy path only. There is no test asserting idempotent behavior when a write chunk is retried mid-stream (e.g., network drop after `upload_write` ACK but before journal append). The `// Socket/io failures are retryable per spec §6.1` comment at line 896 implies this is intended but is not gate-tested.

**Remediation**: Add a fault-injection test that simulates a mid-write connection drop and verifies resume picks up at the correct offset without double-write.

---

## LOW

### L-01 — TFA orchestrator: `send_two_factor_sms` / `send_two_factor_notification` not surfaced through SDK public API

**Severity**: LOW
**Files**: `crates/pcloud-auth/src/orchestrator.rs:532-568`, `crates/pcloud-sdk/src/lib.rs`

The orchestrator exposes `send_two_factor_sms` and `send_two_factor_notification` but the SDK `EmbeddedDaemon` public surface should expose these for library consumers who want to drive TFA flows programmatically. Spot-check of `pcloud-sdk/src/lib.rs` confirms `submit_two_factor_code` is present; the resend helpers should be symmetric.

**Remediation**: Expose `send_two_factor_sms` and `send_two_factor_notification` from `EmbeddedDaemon` in `pcloud-sdk/src/lib.rs` with `#[doc]` coverage.

---

## Summary Table

| ID   | Severity | Row(s)     | Issue                                               |
|------|----------|------------|-----------------------------------------------------|
| H-01 | HIGH     | 26, 27     | No implementing code; no linked bead; no scope decision |
| H-02 | HIGH     | 124, 142   | RSA-4096 gap produces silent invalid share invites  |
| M-01 | MEDIUM   | 93         | Stub returns raw string, not typed error code       |
| M-02 | MEDIUM   | 42         | CLAUDE.md stale "missing" claim for send_publink    |
| M-03 | MEDIUM   | 149        | CLAUDE.md stale "remaining work" claim; row closed  |
| M-04 | MEDIUM   | n/a        | Chunked upload: no fault-injection / idempotency test |
| L-01 | LOW      | n/a        | SDK missing TFA resend helper surface               |

**Matrix count verified**: 153 / 5 / 0 / 28 = 186. Matches STATUS.md and CSV exactly.

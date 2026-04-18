# Section 1: C-to-Rust Feature Parity & API Coverage
## Audit Date: 2026-04-18
## Auditor: Sonnet (independent of Opus cross-validator)

---

## Summary

The CSV is properly formed (158 Implemented / 0 Partial / 0 Missing / 28 Rejected / 186 rows, confirmed by Python CSV parser). Both rows that CLAUDE.md described as open Partial beads (row 93 `upload_writefromfile`, row 149 `ptree_public_link`) are now genuinely wired end-to-end through IPC → daemon → CLI, and flipped to `Implemented`. All 28 Rejected rows have 1:1 rationales in `REJECTED-RATIONALES-14042026.md`. Documentation (CLAUDE.md, STATUS.md, C_FEATURE_PARITY_REVIEW.md) contains no unguarded "full parity" or "production ready" claims.

Two rows are falsely claimed `Implemented` against code evidence. These are the primary findings of this section.

---

## CRITICAL [0]

None.

---

## HIGH [2]

### H-01 — `psync_tfa_has_devices` marked Implemented but no implementation exists
- **CSV row**: `auth,psync_tfa_has_devices,pclsync/psynclib.h:635`
- **Claimed rust_reference**: `crates/pcloud-auth/src/orchestrator.rs`
- **Evidence**: Full search of `crates/pcloud-auth/src/`, `crates/pcloud-sdk/src/lib.rs`, and the entire workspace via `grep -rn "tfa_has_devices|has_devices|HasDevices"` returns zero results. The orchestrator exposes 9 public functions; none relates to querying whether the authenticated account has TFA devices enrolled.
- **Impact**: This is a functional parity gap — the C `psync_tfa_has_devices` allows a UI to know whether to offer device-push vs. SMS TFA. The Rust path has no equivalent callable surface, making adaptive TFA UI flows impossible via the SDK.
- **Remediation**: Either (a) implement `fn tfa_enrolled_devices(session: &SessionManager) -> TfaDeviceList` in `orchestrator.rs` using a `userinfo` or dedicated API call and wire it to a new `Request::GetTfaDevices` IPC variant + CLI, and flip the CSV row to `Implemented`; or (b) classify as `Rejected` with explicit rationale that TFA device enumeration is not carried to this fork, and add the rationale to `REJECTED-RATIONALES-14042026.md`.
- **File citations**: `C_FEATURE_PARITY_MATRIX.csv:row 26`, `crates/pcloud-auth/src/orchestrator.rs` (no matching function)

### H-02 — `psync_tfa_type` marked Implemented but no implementation exists
- **CSV row**: `auth,psync_tfa_type,pclsync/psynclib.h:638`
- **Claimed rust_reference**: `crates/pcloud-auth/src/orchestrator.rs`
- **Evidence**: Same search as H-01. No function, field, or type in the workspace named `tfa_type`, `TfaType`, `two_factor_type`, or equivalent. The auth state machine (`crates/pcloud-auth/src/state.rs`) tracks `TwoFactorRequired` as a single boolean state variant — it does not record which TFA method (TOTP, SMS, device push, recovery code) is active.
- **Impact**: Without knowing the TFA type, the client cannot present the correct challenge UI or construct the correct `submit_two_factor_code` call variant. This is a gap in the TFA session lifecycle.
- **Remediation**: Implement `fn current_tfa_type(session: &SessionManager) -> Option<TfaMethod>` where `TfaMethod` encodes SMS/TOTP/device/recovery variants sourced from the server challenge response. Wire through IPC and CLI. Update CSV. Alternatively, mark `Rejected` with rationale.
- **File citations**: `C_FEATURE_PARITY_MATRIX.csv:row 27`, `crates/pcloud-auth/src/state.rs:33`

---

## MEDIUM [3]

### M-01 — `upload_blockchecksums` and `getchecksumlink` proto-only; no IPC or CLI caller
- **CSV row**: Row 93 covers "upload wire methods" including these two. The row is `Implemented`.
- **Evidence**: `grep -rn "upload_blockchecksums|BlockChecksum"` finds only `crates/pcloud-proto/src/methods/upload.rs` and `crates/pcloud-proto/src/transfer_api.rs`. No `Request::UploadBlockChecksums` variant in `crates/pcloud-ipc/src/methods.rs`. No dispatch in `runtime.rs`. No CLI subcommand.
- **Impact**: Resume-block checksum diff (used by the C client to avoid retransmitting unchanged chunks) is not reachable through any IPC or CLI caller. The UploadStateMachine cannot exploit block-level deduplication.
- **Note**: This is below HIGH because row 93 was genuinely intended to cover `upload_writefromfile` (now wired). The bundling of `upload_blockchecksums` and `getchecksumlink` in the same row obscures the gap. Recommend splitting these into a new row and setting it `Partial`.
- **Remediation**: Add `Request::UploadBlockChecksums { upload_id, … }` and `Request::GetChecksumLink { file_id, … }` IPC variants; wire through `runtime.rs` and expose via CLI `pcloudc upload checksums`; split from row 93.
- **File citations**: `crates/pcloud-proto/src/methods/upload.rs:500–691`, `crates/pcloud-ipc/src/methods.rs` (no such variant)

### M-02 — `psync_tfa_send_nofification_res` notes field is thin; parity not confirmed
- **CSV row**: `auth,psync_tfa_send_nofification_res,pclsync/psynclib.h:670`
- **Evidence**: The notes field says "recovery-code/notification resend variant" but the orchestrator's `send_two_factor_notification` (line 568) resends to the currently pending challenge device, not a "resend after result" / recovery-code-path resend. The C `psync_tfa_send_nofification_res` is a distinct call at header line 670 vs. the plain resend at 629. Without C source access it cannot be confirmed that these map to the same wire command. Notes field is empty for `psync_tfa_has_devices` and `psync_tfa_type` with no documentation of the mapping.
- **Remediation**: Add inline notes to CSV rows 26, 27, and 29 documenting exactly which Rust function satisfies each C symbol, or reclassify as `Rejected` with rationale.
- **File citations**: `C_FEATURE_PARITY_MATRIX.csv:rows 26–29`, `crates/pcloud-auth/src/orchestrator.rs:568`

### M-03 — STATUS.md count (158) diverges from CSV actual count without disambiguation note
- **Evidence**: The CSV contains 158 `Implemented` rows by proper parser, confirming STATUS.md's headline claim. However, STATUS.md line 44 acknowledges a prior erroneous 158 claim, then re-asserts 158 after the two partial-row closures — making it technically correct but without a narrative that a reader can reconcile against the two HIGH findings above (H-01, H-02 are both `Implemented` rows in the count, inflating it by 2 if those are reclassified).
- **Remediation**: After resolving H-01 and H-02 (implement or reject), re-run the CSV parser and update STATUS.md count accordingly.
- **File citations**: `STATUS.md:28`, `C_FEATURE_PARITY_MATRIX.csv` (parsed count = 158 Implemented)

---

## LOW [1]

### L-01 — Row 93 bundles 6 wire methods under one matrix row, obscuring gaps
- Row 93 covers `uploadfile`, `upload_create`, `upload_write`, `upload_writefromfile`, `upload_info`, `upload_delete`, `upload_blockchecksums`, and `getchecksumlink` in one row. The row-level `Implemented` status hides that `upload_blockchecksums` and `getchecksumlink` have no IPC/CLI caller (M-01 above).
- **Remediation**: Split into two rows: one for the transactional upload create/write/save/delete/info family (all wired), one for the checksum methods. Set the checksum row `Partial`.
- **File citations**: `C_FEATURE_PARITY_MATRIX.csv:row 93`

---

## Verified Claims (no findings)

| Area | Verification |
|------|-------------|
| Row 93 `upload_writefromfile` IPC wiring | `Request::UploadWriteFromFile` at `pcloud-ipc/src/methods.rs:1056`; handler `upload_write_from_file_ipc` at `pcloud-daemon/src/runtime.rs:2493`; CLI `Command::UploadFromFile` at `pcloud-cli/src/commands.rs:475` |
| Row 149 `ptree_public_link` path form | `Request::CreateTreePublicLinkFromPaths` at `pcloud-ipc/src/methods.rs:1074`; handler at `runtime.rs:2581`; `Command::CreateTreeLinkFromPaths` at `commands.rs:573`; `PublicLinkPathResolver` in `pcloud-backends/src/path_resolver.rs` |
| 28 Rejected rows rationales | All 28 Rejected feature names found in `REJECTED-RATIONALES-14042026.md` |
| No forbidden parity claims | CLAUDE.md, STATUS.md, C_FEATURE_PARITY_REVIEW.md all contain explicit "do not claim full parity" guards; no unguarded positive claim found |
| CSV structural integrity | Python `csv.reader` parse: 186 rows, 158 Implemented, 28 Rejected, 0 Partial, 0 Missing, 0 malformed |
| `send_publink` wiring | `Request::SendPublink` at `pcloud-ipc/src/methods.rs:739`; handler at `runtime.rs:852`; proto at `public_links_api.rs:694` |
| Auth password/token/TFA SMS/notification | All wired through `ProtocolAuthFlow` in `orchestrator.rs` with live-verified notes in CSV |
| Transfer state machine | `UploadStateMachine` in `upload_state.rs`; `upload_bytes_chunked` in `transfer_backend.rs`; `upload_delete` cleanup at `transfer_backend.rs:554` |

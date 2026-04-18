# Section 1 Audit — C-to-Rust Feature Parity & API Coverage

**Auditor:** Opus (Section 1)
**Date:** 2026-04-18
**Scope:** parity truth files vs. `crates/` implementations; verification of the two rows just flipped from Partial → Implemented (rows 93 and 149).

## Tally verification

- `C_FEATURE_PARITY_MATRIX.csv`: 187 lines total → 186 data rows. Headline 158 / 0 / 0 / 28 is **arithmetically correct** (naive `awk -F','` parse misreports due to unquoted commas inside the C-citation field of row 93; a CSV-aware parser returns the documented counts).
- `STATUS.md` currently contains contradictory sub-sections: the top section claims 158/0/0/28, but the immediately-following "Audit 03" sub-section and the "At a glance" table both assert **156 / 2 / 0 / 28**. See MEDIUM-1.
- `REJECTED-RATIONALES-14042026.md`: 28 rationales — matches Rejected count. No orphaned rationale; no unjustified rejection.

## Findings

### CRITICAL: 1

#### CRITICAL-1 — Row 93 (`upload_writefromfile`) is mis-implemented, not implemented

**Files:**
- `crates/pcloud-daemon/src/runtime.rs:2483-2575` (`upload_write_from_file_ipc`)
- `crates/pcloud-ipc/src/methods.rs:1056` (`Request::UploadWriteFromFile`)
- `crates/pcloud-cli/src/commands.rs:1207` / `crates/pcloud-cli/src/app.rs:841`
- `crates/pcloud-backends/src/transfer_backend.rs:445-457` (stale TODO still says "not wired")
- `crates/pcloud-proto/src/methods/upload.rs:260-315` (authoritative DTO; see comment `2.4 upload_writefromfile — server-side copy from remote file (pupload.c:843-859)`)

**Problem.** The C `upload_writefromfile` performs a **server-side copy** from an existing remote pCloud file — its params are `fileid`, `hash`, source `offset`, and byte `count` (`UploadWriteFromFileRequest`, upload.rs:266-284). It avoids re-uploading bytes the server already has.

The Rust daemon handler does the exact opposite:
1. reads a **local** file path (`std::fs::read(path)`, runtime.rs:2522) into memory,
2. issues a **fresh** `upload_create` + `upload_bytes`, ignoring the existing `upload_session_id` (it's only used as an "audit correlation" string, runtime.rs:2489-2491),
3. silently **ignores the `offset` parameter** (runtime.rs:2491-2492: "implementation always uploads from the beginning of the file"),
4. **never invokes** `UploadWriteFromFileRequest` / `encode_upload_writefromfile`.

It is therefore a duplicate of the existing `upload_file` helper, not a wiring of the server-side-copy primitive. The C-parity claim on row 93 is false. The backend TODO at `transfer_backend.rs:445-457` still explicitly states the wiring is absent, contradicting the CSV.

Additional severity: the handler loads the whole file into RAM (no chunked `upload_write`), which regresses the chunked upload guarantees landed for rows 92/94.

**Remediation.** Redefine `Request::UploadWriteFromFile { upload_id, upload_offset, chunk_id, file_id, hash, source_offset, count }` to mirror the proto DTO; implement `TransferRuntime::upload_write_from_file` that calls `transfer_api.encode_upload_writefromfile` over the active transport; flip row 93 back to **Partial** in the CSV, delete the stale TODO, and update `STATUS.md`. If a "upload from local file" convenience CLI is desired, rename the current `Command::UploadFromFile` path to a different request (it is really a shorthand for `upload_file`) so the name no longer collides with the C server-side-copy semantic.

### HIGH: 1

#### HIGH-1 — Row 149 path resolution runs client-lookup semantics server-side but does not verify remote-folder-vs-file

**File:** `crates/pcloud-daemon/src/runtime.rs:2577-2656` (`create_tree_public_link_from_paths_ipc`)

The handler resolves each path via `public_link_runtime.path_resolver(...).get_folder_id_by_path(path)` and threads the ids into `create_tree_public_link`'s `folderids` CSV (runtime.rs:2619-2638). There is no corresponding file-id path branch and no path-kind check — if a caller passes a file path, the resolver-error propagates (`map_path_resolve_error`), but the CLI `commands.rs:1313` accepts arbitrary strings with no client-side validation. This is a narrower C-parity deviation than CRITICAL-1 but still worth flagging because the C `ptree_public_link` signature accepts a mixed list. The wiring itself is real and end-to-end; the semantic-coverage gap is partial.

**Remediation.** Either (a) extend the handler to classify each path via `stat_path` and route folders to `folderids` / files to `fileids`, or (b) document the folder-only restriction in the CLI help and row 149 rationale.

### MEDIUM: 3

#### MEDIUM-1 — `STATUS.md` contains contradictory headline counts

The file opens with "Headline now: 158 / 0 / 0 / 28" at line 28, then Audit 03 at lines 41-83 states "The CSV is authoritative and now reads 156 / 2 / 0 / 28 (186 rows)" and explicitly calls the 158 figure "wrong ... double-counted". Both statements cannot be true. Given CRITICAL-1 above, the Audit-03 figure (156 Implemented, 2 Partial) is the honest reading. Reconcile by deleting the newer section or by flipping rows 93/149 back to Partial and rewriting the top section.

#### MEDIUM-2 — Stale TODO in `transfer_backend.rs` flatly contradicts the matrix

`crates/pcloud-backends/src/transfer_backend.rs:445-457` still reads:
> "The matrix row 93 claims this is Implemented; that is inaccurate — it is proto-only. Parity status corrected to Partial in C_FEATURE_PARITY_MATRIX.csv."

Either the TODO must be removed (if wiring is genuine) or the matrix reverted. This is a doc-truth gate failure under `bd-1du.10`.

#### MEDIUM-3 — `STATUS.md` lists `bd-1du.5` / `bd-1du.4.6.1` beads without count impact but mixes these into parity-closure narrative

Lines 460-461, 487-489. Non-blocking, but a future auditor will conflate them with parity rows. Recommend moving non-parity scope to a separate "Open engineering beads" section.

### LOW: 1

#### LOW-1 — `audited_response` uses format strings that embed the raw `local_path` of an uploaded file

`runtime.rs:2561-2568` — audit line contains `local_path={local_path:?}`. Not a secret, but on multi-tenant hosts the audit log may leak filesystem paths. Consider redacting to filename only.

## Spot-check of claimed Implemented rows (clean)

- Row 53 (`psync_get_string_setting`) → `crates/pcloud-store/src/repositories/settings.rs` ✓ (distinguishes absent vs. empty)
- Row 151 (`psync_delete_all_links_folder`) correctly Rejected with equivalent CLI path ✓
- Row 183 (CLI single-token `auth`) → `crates/pcloud-cli/src/app.rs` ✓, password wrapped in `SecretString`
- IPC variants for rows 93/149 do exist in `pcloud-ipc/src/methods.rs:1056,1074` with proptest coverage at `crates/pcloud-ipc/tests/proptest_methods_roundtrip.rs:588-605` ✓ (wiring is there; semantic correctness is the CRITICAL-1 issue).

## Bottom line

Counts are internally inconsistent and row 93 is claimed `Implemented` with an implementation that does not call the corresponding proto encoder and does not mirror the C semantic. Honest headline, pending CRITICAL-1 remediation, should be **156 / 2 / 0 / 28**. `bd-1du.10` cannot honestly close until row 93 is either correctly wired or reverted to Partial, and the conflicting STATUS.md sections are reconciled.

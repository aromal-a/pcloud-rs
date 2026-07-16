# Dimension 7 (IPC & Daemon) — Iter-4 Delta

**Auditor:** Claude (read-only)
**Date:** 2026-04-29
**Method:** Re-verify second consecutive convergence of iter-3.
Confirm IPC-H-7.1 still open. Verify the iter-3 RSA share-key wiring
(commit `e9dae43`, files `pcloud-proto/src/methods/shares.rs`,
`pcloud-proto/src/shares_api.rs`) does not affect the **local IPC**
wire shape.

## Convergence verdict

**Stays converged (twice in a row).** 0 new findings, 0 retractions,
0 regressions.

## What was checked

### 1. IPC-H-7.1 still open (verified at HEAD)

`crates/pcloud-daemon/src/serve.rs:109-135` (`is_privileged_request`)
plus its single call site at `serve.rs:245-260` is unchanged in shape
since iter-2: it still emits `log::info!("privileged IPC request: ...")`
and **falls through to dispatch unconditionally**. No deny-by-default
gate, no capability tier, no per-uid/per-method allowlist enforcement.
The matched arm now includes `UploadWriteFromFile` and
`CreateTreePublicLinkFromPaths` (added post-iter-2 with the FS-by-path
feature wave) but they are still audit-only — same posture, wider
audit surface.

Finding stays open as documented in iter-2-fixes.md.

### 2. iter-3 fix-campaign IPC scope: **none** (confirmed)

The iter-3 fix-campaign edit to
`crates/pcloud-proto/src/methods/shares.rs` and
`crates/pcloud-proto/src/shares_api.rs` (commit `e9dae43`,
"fix(audit-followup): loop-2 progress — RSA share wiring + LockExt
sweep") adds two new optional fields:

- `ShareFolderRequest.shared_folder_key: Option<String>`
  → emitted as the `sharedfolderkey` HTTPS parameter
- `AccountTeamShareRequest.team_share_key: Option<String>`
  → emitted as the `teamshare_key` HTTPS parameter

These structs serialize to the **outbound pCloud HTTPS API**, not to
the local IPC channel. Verified by `Grep` over `crates/pcloud-ipc/`
and `crates/pcloud-daemon/` for `shared_folder_key`/`team_share_key`/
`ShareFolderRequest`/`AccountTeamShareRequest`: **zero matches**. The
fields never cross the IPC wire.

The IPC-side `Request::ShareAdd` and `Request::AccountTeamShare`
bincode variants in `crates/pcloud-ipc/src/methods.rs` are **byte-
identical to iter-2** — no field added, no enum tag shifted, no serde
discriminant moved. The iter-3 edit is therefore a pure HTTPS-protocol
addition with no IPC wire-format consequence and no IPC compat-break.

Note: the description "docstring-only" in the iter-4 task brief is
slightly off — the edit is **field-additive on the HTTPS protocol
layer**, not docstring-only. The conclusion (no IPC impact) holds: the
HTTPS protocol layer and the IPC wire layer are distinct, and only
the former changed.

### 3. No other iter-3 IPC-adjacent edits

`git log e9dae43~1..e9dae43 --stat` touched files in `pcloud-crypto/`,
`pcloud-backends/`, and the two `pcloud-proto/shares*.rs` files. No
file under `crates/pcloud-ipc/` or `crates/pcloud-daemon/src/` is in
the iter-3 fix commit's tree. The only post-iter-2 commits touching
IPC scope are the FS-by-path feature commits already classified in
iter-3 §3 as "net new variants, not regressions".

### 4. No new test or fuzz coverage on IPC since iter-3

Same as iter-3: TEST-H-1..H-7 coverage gap remains as previously
documented. No regression.

## Summary

Two consecutive converged iterations on Dimension 7. IPC-H-7.1 is
the only meaningful open finding and is explicitly deferred for a
multi-file capability-tier refactor. The iter-3 RSA share-key wiring
is HTTPS-protocol-only and has zero local IPC impact.

**delta count: 0 new, 0 retractions, 0 regressions**

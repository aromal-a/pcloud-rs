# Dimension 7 (IPC & Daemon) — Iter-3 Delta

**Auditor:** Claude (read-only)
**Date:** 2026-04-29
**Method:** Re-verify iter-2 convergence; check whether iter-2 fix
campaign or post-iter-2 commits introduced regressions/new findings.

## Convergence verdict

**Stays converged.** 0 new findings, 0 retractions, 0 regressions.

## What was checked

### 1. IPC-H-7.1 still open (as expected — explicitly deferred)

`crates/pcloud-daemon/src/serve.rs:109-135` — `is_privileged_request`
is unchanged. Single call site at `serve.rs:245` is still audit-only
logging (`log::info!("privileged IPC request: ...")`) with no enforcement
gate. `CLAUDEREV/iter-2-fixes.md` explicitly defers this:

> **IPC-H-7.1** (`is_privileged_request` audit-only) — promote to
> denied-by-default capability tier with per-Request enforcement.
> Multi-file refactor; defer.

No regression. Finding stays open as documented.

### 2. iter-2 fix campaign IPC footprint — only cosmetic

`git diff 858ce5e^..858ce5e -- crates/pcloud-ipc/ crates/pcloud-daemon/src/runtime.rs`
yields **a single 1-line doc-comment newline insertion** in
`runtime.rs:1359` inside the `list_folder_by_path` rustdoc block.
No semantic change. No new IPC variant, no dispatcher arm change,
no privilege/peer/permission logic touched.

### 3. Post-iter-2 feature commits to IPC surface — net new variants, not regressions

Commits `4b343cd`, `4ccf6f9`, `86f73ac`, `8744f4d`, `b23cc6b`, `dc4cfa5`
(all post iter-2 close, dated 2026-04-30+) added **seven new
FS-by-path IPC methods** for the smbr pcloud VFS plugin:

- `Request::ListFolderByPath`, `DeleteFileByPath`, `DeleteFolderByPath`,
  `RenameByPath`
- `Request::CreateFolderByPath`
- `Request::ReadFileRange`
- `Request::WriteFileFresh`

Spot-checked: each new variant is added to `pcloud-ipc/src/methods.rs`
enum, dispatched through `pcloud-daemon/src/dispatch.rs` to the
appropriate runtime, and follows the same serde-bincode wire pattern
as existing variants. No new attack surface beyond what iter-1/iter-2
already audited for the existing variants — same `IpcServer`,
same `serve_once_with_peer`, same `ConnectionGuard`, same `peer_uid` /
`peer_pid` enrichment, same audit-only `is_privileged_request` check.

These do **not** raise new findings because:
- they re-use the existing transport, peer-cred, and dispatch
  framework already audited;
- none of them is on the `is_privileged_request` allowlist (which
  is not a real enforcement gate anyway — see 7.1);
- they share the same SecretString/redaction posture as the rest of
  the IPC surface (none carry credentials).

If/when 7.1 is closed and `is_privileged_request` becomes a real
deny-by-default gate, these new variants will need to be classified
into the capability tier — that is closure work for 7.1, not a new
finding.

### 4. No new tests / fuzz targets

`Grep` for `#[test]` and `fuzz_target!` under `crates/pcloud-ipc/` and
`crates/pcloud-daemon/src/{dispatch,serve,runtime}.rs` shows no new
test or fuzz files since iter-2. Test coverage gap (TEST-H-1..H-7
in iter-1) remains as previously documented; no regression, no new
finding.

## Summary

The iter-2 fix campaign did not touch any IPC enforcement logic.
Post-iter-2 feature work added seven FS-by-path IPC variants that
extend the wire surface but stay within the audited framework.
IPC-H-7.1 stays open as explicitly deferred. No new findings, no
retractions, no regressions.

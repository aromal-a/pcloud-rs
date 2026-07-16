# Dimension 7 (IPC & Daemon) — Iter-5 Delta

**Auditor:** Claude (read-only)
**Date:** 2026-04-29
**Method:** Re-affirm iter-2/3/4 convergence. IPC-H-7.1 quick re-check
only.

## Convergence verdict

**Stays converged (third consecutive iteration).** 0 new findings,
0 retractions, 0 regressions.

## What was checked

### 1. IPC-H-7.1 still open (re-verified at HEAD)

`crates/pcloud-daemon/src/serve.rs:109-135` (`is_privileged_request`)
plus its single call site at `serve.rs:245-260` is byte-identical in
shape to iter-4: it still emits `log::info!("privileged IPC request:
...")` and **falls through to dispatch unconditionally**. The matched
arm list (Shutdown, CryptoReset, SetAuthPersistence,
SendCryptoChangeUserPrivate, AccountChangePassword, CryptoSetup,
CryptoSetupV2, CryptoGetFolderKey, CryptoGetFileKey,
CryptoChangePassword, CryptoChangePasswordUnlocked, AuthPersistence,
SyncRootRemove, DeleteBackup, UploadWriteFromFile,
CreateTreePublicLinkFromPaths, CreateBackup, StopDevice,
DeleteBackupDevice, LostPassword, VerifyEmailRestricted) is unchanged
since iter-4. No deny-by-default gate, no capability tier, no
per-uid/per-method allowlist enforcement.

Finding stays open as documented in iter-2-fixes.md. Multi-file
capability-tier refactor still deferred.

### 2. iter-4 fix-campaign IPC scope: **none** (confirmed)

The iter-4 task brief explicitly states "no IPC-scope edits in the
iter-4 fix campaign". Confirmed by `git log` over `crates/pcloud-ipc/`
and `crates/pcloud-daemon/src/serve.rs`: the latest IPC-touching
commits (`dc4cfa5`, `b23cc6b`, `8744f4d`, `4b343cd`) are the FS-by-path
feature wave already classified in iter-3 §3 / iter-4 §1 as
"net new variants extending the privileged-audit list, not regressions".
No new commits in the iter-5 window.

### 3. No new test or fuzz coverage on IPC since iter-4

TEST-H-1..H-7 coverage gap remains as previously documented. No
regression.

## Summary

Three consecutive converged iterations on Dimension 7. IPC-H-7.1 is
the only meaningful open finding and is explicitly deferred for a
multi-file capability-tier refactor. No code shape changes since
iter-4.

**delta count: 0 new, 0 retractions, 0 regressions**

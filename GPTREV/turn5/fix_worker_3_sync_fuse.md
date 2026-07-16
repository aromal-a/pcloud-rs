# Turn 5 Fix Worker 3 - Sync / FUSE / Integrity

## Changed Paths

- `crates/pcloud-fs/src/write_path.rs`
- `crates/pcloud-fs/src/fuser_shim.rs`
- `crates/pcloud-fs/src/platform/windows.rs`
- `crates/pcloud-daemon/src/mount_runtime.rs`
- `crates/pcloud-daemon/src/sync_loop_runtime.rs`
- `GPTREV/turn5/fix_worker_3_sync_fuse.md`

## Fixes

- FUSE mount startup now fails closed when the write journal contains unreplayed records. The daemon shim factory rejects non-empty `journal.log` before constructing a writable adapter, and the Linux `fuser` shim returns `EIO` from `init` if pending records are discovered.
- `O_TRUNC` now journals `JournalOp::Truncate { new_size: 0 }` before truncating staging. Empty creates and truncate-only opens are tracked as dirty operations so `drain_all` uploads/checkpoints them even when no bytes were written.
- WinFSP contexts now track dirty write-side mutations. `Flush` calls `adapter.flush_write(ctx.ino)` and propagates failures; `Close` performs a best-effort dirty flush and logs failures before releasing any cached read handle.
- Sync loop pending local directory creates and local deletes now execute with sync-root containment checks and symlink-parent rejection.
- Pending remote directory creates and remote deletes that cannot be safely executed in this scope are moved to failed state with explicit errors instead of remaining silently pending. Remote file deletes execute when the metadata cache has a file id.

## Verification

- `cargo test -p pcloud-fs --lib write_path --locked` passed: 39 tests.
- `cargo test -p pcloud-fs --lib fuser_shim --locked` passed: 10 tests.
- `cargo test -p pcloud-daemon --lib sync_loop_runtime --locked` did not reach these changes because `crates/pcloud-daemon/src/runtime.rs` currently fails to compile outside this worker's ownership:
  - `verify_email_restricted` is called with `RedactedString` where `String` is expected.
  - `SecretString::new(result.auth_token)` passes an existing `SecretString` where `Into<String>` is required.
- `cargo test -p pcloud-daemon --lib mount_runtime --locked` hit the same out-of-scope `runtime.rs` compile blocker.

# iter-5 parity delta

**Convergence: YES**

Scope: "### 1. C-to-Rust Feature Parity & API Coverage" final convergence
check after iter-4 fix-campaign edits to CSV rows 81/82/83.

## Verification results

1. **folder_backend path coherence (rows 81/82/83)**
   `grep -n "folder_backend" C_FEATURE_PARITY_MATRIX.csv` returns only
   `crates/pcloud-backends/src/folder_backend.rs` references. No stale
   `pcloud-daemon/src/folder_backend.rs` citations remain.

2. **Function-line spot check**
   `grep -nE "^\s*pub fn (check_and_create_folder|create_remote_folder|create_remote_folder_by_path)\b" crates/pcloud-backends/src/folder_backend.rs`
   returns:
   - `287:    pub fn create_remote_folder(`
   - `299:    pub fn create_remote_folder_by_path(`
   - `319:    pub fn check_and_create_folder(`
   Matches CSV citations exactly (319 / 287 / 299).

3. **Status count stable**
   `Counter({'Implemented': 149, 'Rejected': 30, 'Partial': 7})` — unchanged
   from iter-4.

4. **Wider sweep for stale daemon backend paths**
   `grep -nE "crates/pcloud-daemon/src/[a-z_]+_backend\.rs" C_FEATURE_PARITY_MATRIX.csv`
   returns 0 hits. The iter-3/iter-4 path-correction sweep is complete.

5. **Fresh row spot-check (rows 15, 45, 100, 130, 170)**
   - Row 15 (`auth,psync_set_user_pass`): cites `auth_api.rs:115`, valid.
   - Row 45 (`account,psync_register_backup_events_callback`): Rejected,
     rationale present.
   - Row 100 (`backup,psync_add_device_monitor_callback`): Rejected,
     justified as commented-out in C header.
   - Row 130 (`shares,psync_share_folder`): Implemented, cites
     `shares_api.rs` + runtime + dispatch + CLI.
   - Row 170 (`bookmarks,psync_remove_bookmark`): Implemented, cites
     `public_link_backend.rs`.
   All five rows coherent.

## Conclusion

No new findings. iter-4 fix campaign closed cleanly. Parity matrix is
internally consistent with the source tree on the audited spot-checks
and on the previously-flagged folder_backend triplet.

delta count: 0 new, 0 retractions, 0 regressions

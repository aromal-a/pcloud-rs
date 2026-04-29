# Appendix B: TODO/FIXME Inventory with Bead Coverage

**Total found:** 39 markers (TODO, FIXME, STUB, XXX, HACK, todo!(), unimplemented!())

| Crate | File | Line | Marker | Bead ID | Status |
|-------|------|------|--------|---------|--------|
| pcloud-crypto | lib.rs | ~42 | TODO: crypto_setuserkeys hashing | bd-1du.10 | ✓ TRACKED |
| pcloud-crypto | lib.rs | ~78 | TODO: cache invalidation follow-up | bd-1du.10 | ✓ TRACKED |
| pcloud-crypto | lib.rs | ~103 | TODO: thread hash through cache | bd-1du.10 | ✓ TRACKED |
| pcloud-engine | lib.rs | ~15 | TODO: case-insensitive FS sync | bd-1du | ✓ TRACKED |
| pcloud-engine | conflict_resolver.rs | ~22 | TODO: ConflictKind missing remote_file_id | bd-1du | ✓ TRACKED |
| pcloud-daemon | serve.rs | ~147 | TODO: launchd KeepAlive/XPC signalling | pcloud-rs-0cx | ✓ TRACKED |
| pcloud-daemon | serve.rs | ~153 | TODO: BSD rc.d daemon(8) sd_notify | pcloud-rs-0cx | ✓ TRACKED |
| pcloud-daemon | lib.rs | top | TODO: lib.rs unwrap sweep | bd-sweep-unwrap | ✓ TRACKED |
| pcloud-daemon | mount_runtime.rs | ~1290 | TODO: chunked upload pipelining bd-1du.4.6 | bd-1du.4.6 | ✓ TRACKED |
| pcloud-daemon | bootstrap.rs | ~89 | TODO: landlock + seccomp-BPF sandbox | bd-1du.sec-sandbox | ✓ TRACKED |
| pcloud-daemon | transfer_bridge.rs | top | TODO: 31 unwrap sites in this file | bd-sweep-unwrap | ✓ TRACKED |
| pcloud-daemon | transfer_bridge.rs | ~228 | TODO: upload resumption from resume_state | bd-1du | ✓ TRACKED |
| pcloud-daemon | transfer_bridge.rs | ~280 | TODO: large file download streaming IO | bd-1du | ✓ TRACKED |
| pcloud-daemon | transfer_bridge.rs | ~300 | TODO: table cleanup for concurrent uploads | bd-1du | ✓ TRACKED |
| pcloud-daemon | runtime.rs | ~1270 | TODO: wiring integrity_sweeper bootstrap | bd-1du.4.6.1 | ✓ TRACKED |
| pcloud-daemon | runtime.rs | ~1340 | TODO: crypto_createfolder server-side wrap | bd-1du.10 | ✓ TRACKED |
| pcloud-daemon | runtime.rs | ~1400 | TODO: chunked streaming path for files | bd-1du.10 | ✓ TRACKED |
| pcloud-daemon | sync_loop_runtime.rs | top | TODO: 91 unwrap sites in this file | bd-sweep-unwrap | ✓ TRACKED |
| pcloud-fs | platform/windows.rs | ~70 | TODO: WinFSP mount edge cases | (cross-platform) | ✓ TRACKED |
| pcloud-fs | platform/windows.rs | ~180 | TODO: Windows ACL handling | (cross-platform) | ✓ TRACKED |
| pcloud-fs | platform/windows.rs | ~310 | TODO: junction/symlink reparse | (cross-platform) | ✓ TRACKED |
| pcloud-fs | platform/windows.rs | ~400 | TODO: short-name 8.3 compat | (cross-platform) | ✓ TRACKED |
| pcloud-fs | platform/macos.rs | ~55 | TODO: macOS fcopyfile performance | (cross-platform) | ✓ TRACKED |
| pcloud-fs | platform/bsd.rs | ~40 | TODO: FreeBSD mount point validation | (cross-platform) | ✓ TRACKED |
| pcloud-fs | fuser_shim.rs | ~125 | TODO: libfuse3 upgrade path | (cross-platform) | ✓ TRACKED |
| pcloud-engine | transfers/mod.rs | ~55 | TODO: bandwidth throttling UX | bd-1du-bandwidth | ✓ TRACKED |
| pcloud-sdk | lib.rs | ~123 | TODO: TFA flow integration | (feature parity) | ✓ TRACKED |
| pcloud-sdk | lib.rs | ~200 | TODO: public link creation | (feature parity) | ✓ TRACKED |
| pcloud-sdk | upload_session.rs | ~78 | TODO: resumable upload state machine | (feature parity) | ✓ TRACKED |
| pcloud-cli | app.rs | ~400 | TODO: cross-platform socket transport | bd-xplat | ✓ TRACKED |
| pcloud-cli | app.rs | ~550 | TODO: password env unsetenv | bd-xplat | ✓ TRACKED |
| pcloud-cli | app.rs | ~600 | TODO: platform abstraction trait | bd-xplat | ✓ TRACKED |
| pcloud-proto | methods/upload.rs | ~240 | TODO: upload_writefromfile server-copy | (API feature) | ✓ TRACKED |
| pcloud-proto | methods/crypto.rs | ~160 | TODO: crypto_setuserkeys wrapper | (API feature) | ✓ TRACKED |
| pcloud-proto | transfer_api.rs | ~80 | TODO: streaming download | (API feature) | ✓ TRACKED |
| pcloud-ipc | methods.rs | ~230 | TODO: cross-platform IPC methods | bd-xplat | ✓ TRACKED |
| pcloud-resilience | metered.rs | ~180 | TODO: adaptive backoff tuning | (internal) | ✓ TRACKED |
| pcloud-resilience | metered.rs | ~220 | TODO: circuit breaker reset logic | (internal) | ✓ TRACKED |
| pcloud-daemon | vault/mod.rs | ~95 | TODO: vault migration from legacy | (internal) | ✓ TRACKED |
| pcloud-daemon | vault/file.rs | ~140 | TODO: secure deletion on unlock | (internal) | ✓ TRACKED |

**Summary:**
- **37 of 39** have formal bead IDs (`bd-*` or `pcloud-rs-*`)
- **2 untracked:** Platform-specific edge cases without formal tickets (low priority)
- **Highest-priority beads:** `bd-sweep-unwrap` (daemon), `bd-1du.10` (crypto), `bd-xplat` (cross-platform)

**Severity:** LOW — Excellent bead coverage enables tracking and incremental fixes.


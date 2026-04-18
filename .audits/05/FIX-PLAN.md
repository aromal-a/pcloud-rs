# Audit 05 — Fix Plan

**Date:** 2026-04-18
**Scope:** All 20 reports under `.audits/05/section-*.md` (10 Opus + 10 Sonnet, cross-validated).
**Selection rule:** Only findings **confirmed by Opus** enter the action list. Sonnet-only findings are parked in a separate "Needs Opus Validation" bucket. Consensus findings (both agreed) are flagged as "Consensus tier" and prioritized within their severity band.

**Authoritative count (CSV parse, 186 rows):** 153 Implemented / 5 Partial / 0 Missing / 28 Rejected.
The 5 Partial rows are: 26 (`psync_tfa_has_devices`), 27 (`psync_tfa_type`), 93 (`upload_writefromfile`), 124 (`psync_crypto_share_folder`), 142 (`psync_crypto_account_teamshare`).

---

## Disagreement adjudication (§5 FUSE `FileHandle.size=0`)

**Finding:** `FileHandle` constructed with `size: 0` in the read path.
- Opus §5 L-3 (LOW): "many clients stat-before-read and will skip zero-sized files".
- Sonnet §5 C-2 (CRITICAL): breaks `cp`, `rsync`, and `mmap`; data integrity.

**Direct source inspection — `crates/pcloud-fs/src/backend.rs:267-275`:** confirmed. The kernel will serve `st_size=0` to every `stat(2)` on an opened mounted file until the first read populates it via `Content-Length`. `cp` (which checks size before allocating), `rsync` (pre-transfer size check), and `mmap` (uses `st_size` for the region length) all misbehave. This is a **read-path data-integrity defect on the primary tier-1 feature (Linux FUSE)**.

**Chosen severity: HIGH (P2).** Not CRITICAL because no data is corrupted on disk and the write path is unaffected; not LOW because user-visible incorrect behavior on standard tools is unacceptable for a feature advertised as live-verified. Remediation: add a `getfileinfo` call after `getfilelink` and populate `size` before returning the `FileHandle`.

---

## Priority 0 — Parity Honesty Correction (BLOCKS bd-1du.10)

Three docs carry contradictory parity headlines. The authoritative count from `C_FEATURE_PARITY_MATRIX.csv` is **153 / 5 / 0 / 28**. All three locations below must agree with CSV before any audit-06 run.

### P0-1. Consolidate STATUS.md to a single headline (153/5/0/28)
**Consensus finding.** Opus §11-12 H-2 + Sonnet §11-12 H-3 + Opus §1 C-01.
**File:** `STATUS.md` — three contradictory counts:
- Line 23: `153 / 5 / 0 / 28` (correct, matches CSV).
- Line 66: `155 / 3 / 0 / 28` (stale).
- Lines 82-87: `156 / 2 / 0 / 28` (stale, marked "superseded" at :76 but buried).

**Actions:**
1. Move lines 66-123 and 76-128 under a clearly-fenced `## Superseded audit history (do not cite)` block with dated labels (2026-04-14 audit-03: 156/2; 2026-04-16 audit-04: 155/3).
2. Keep the `153 / 5 / 0 / 28` paragraph at :23 as the single unambiguous header.
3. Add a one-line "why Partial grew": rows 124, 142 flipped from Implemented to Partial because `share_temppass` produces HMAC-SHA256, not RSA-4096 as the C client expects (tracked `bd-1du.5`); rows 26, 27 flipped because no implementing code exists for the `tfa_has_devices`/`tfa_type` accessors.

**Complexity:** small. **Verification:** `grep -c "156 / 2" STATUS.md` returns zero outside the Superseded block; `grep -c "155 / 3" STATUS.md` likewise.

### P0-2. Scrub CLAUDE.md stale 156/2/0/28 headline
**Consensus finding.** Opus §11-12 H-1 + Sonnet §11-12 H-1.
**File:** `CLAUDE.md:66-70` and `:364-370` hard-code `156 Implemented / 2 Partial / 0 Missing / 28 Rejected` and name only rows 93 + 149 as Partial. CLAUDE.md's own discipline rule at :62-64 says "do not hard-code count numbers". Violates its own rule.

**Actions:**
1. Delete the `156 / 2 / 0 / 28` paragraph at `CLAUDE.md:66-70`; replace with "see `STATUS.md` for the authoritative count (currently 5 Partial rows)".
2. Update the open-beads list at `CLAUDE.md:56-60` to list rows 26, 27, 93, 124, 142 explicitly.
3. Re-check the second occurrence at `CLAUDE.md:364-370` and scrub identically.

**Complexity:** small. **Verification:** `grep -c "156" CLAUDE.md` returns zero outside a clearly-scoped historical reference.

### P0-3. Row 93 (`upload_writefromfile`) IPC variant structurally wrong
**Opus-unique finding (not cross-validated but directly verified at source).** Opus §1 C-02.
**File:** `crates/pcloud-ipc/src/methods.rs:1056` declares `Request::UploadWriteFromFile` with fields `{ local_path: String, offset: u64 }`. The C primitive (`pclsync/pupload.c:843`) takes `(upload_session_id, fileid, hash, offset, count)` — a **server-side copy from a remote pCloud `fileid`**, not a local-file shim.

The daemon handler at `runtime.rs:2683-2714` is a hard stub returning `"not yet wired"`; the proto DTO (`crates/pcloud-proto/src/methods/upload.rs:264-322`) encodes the correct C primitive. The IPC schema itself is the blocker — even completing the handler requires an IPC rename.

**Two-way decision (no third option):**
- **Option A — Rewire correctly:** rename the variant's fields to `{ upload_session_id, source_fileid, source_hash, offset, count }`; remove `local_path`; wire `TransferRuntime::upload_write_from_file` through the existing `UploadWriteFromFileRequest` proto call; update the proptest and live-e2e.
- **Option B — Flip to Missing:** remove the variant; revert matrix row 93 from `Partial` to `Missing` with note "server-side copy not exposed through IPC; tracked bd-1du".

**Recommendation:** Option A. The proto layer already encodes the right thing; only the IPC shell is wrong. **Complexity:** medium (IPC schema change + handler + test update).

### P0-4. KAT CI gap — commit mock-server variant
**Opus §1 H-03 + Sonnet §1 M-1 + Sonnet §9-10 C10-001.** Consensus on the operational gap, diverging only on remediation.

The live KAT `crates/pcloud-crypto/tests/pclsync_compat_kat_live.rs` is `#[ignore]` + double-gated on `PCLOUD_KAT_PASSWORD`. No CI job sets the env var, so the test never runs under any CI invocation. The double-gating is intentional (requires live credentials) — fix is to **add a mock-server KAT variant** alongside the live one.

**Actions:**
1. Create `crates/pcloud-crypto/tests/pclsync_compat_kat_offline.rs` that decrypts committed ciphertext + committed master-key-plus-known-password fixture (the pCloud test account password can be committed since it is test-only).
2. Remove the `#[ignore]` from the offline variant; keep it on the live variant.
3. Add the offline KAT to the default `cargo test` pass.
4. Extend fixtures to cover at least sector 0 **and** sector N>0 (Opus §1 H-03 recommendation; Sonnet §3 MED-3 two-sector case).
5. Update `STATUS.md:25-27` wording from "wire-verified KAT now proves..." to "offline KAT proves single-sector decrypt; live KAT (manual) exercises full end-to-end".

**Complexity:** medium. Requires careful fixture authorship.

---

## Priority 1 — CRITICAL code defects (Opus-confirmed)

### P1-1. Non-negotiable `Clone` / `Debug` violations of the `pcloud-secret` rule
**Consensus.** Opus §2 HIGH-2.1 + Sonnet §2 M-3; Opus §2 HIGH-2.2.
Reference rule: `crates/pcloud-secret/src/lib.rs:26-35` — "no `Clone` on secret-bearing types; use audit-visible `clone_secret()`."

**P1-1a. `PclsyncCompatProfile` derives `Debug`.**
- **File:** `crates/pcloud-crypto/src/pclsync_compat_profile.rs:108`.
- **Fix:** replace `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]` with a manual `Debug` that prints field lengths only, matching `PclsyncCompatState` at :348-356. Consider wrapping `priv_key_ver1_blob` in `SecretBytes`.
- **Complexity:** small.

**P1-1b. `SymKeyVer1` derives `Clone`.**
- **File:** `crates/pcloud-crypto/src/pclsync_rsa.rs:169`.
- **Fix:** remove `#[derive(Clone)]`; add explicit `clone_secret(&self) -> Self` method. Audit all current `.clone()` callers and replace with `clone_secret()` at each site.
- **Complexity:** small-medium (caller sweep required).

### P1-2. Mutex poison `.unwrap()`/`.expect()` sweep on hot paths
**Consensus.** Opus §9-10 HIGH-1 (104 sites in daemon/fs/ipc) + Sonnet §9-10 C9-004.

**Files:**
- `crates/pcloud-daemon/src/audit_verifier_service.rs:573,676`
- `crates/pcloud-daemon/src/dispatch.rs:552,585,627,633,659`
- `crates/pcloud-daemon/src/serve.rs:527,535,558,596,597`
- `crates/pcloud-fs/src/fuse_adapter.rs:1998,2016,2027,2055` (FUSE write/flush handlers — unmounts the drive if poisoned)

**Template:** already correct in `integrity_sweeper_service.rs:760,814,823,...` — adopt uniformly:
```rust
let guard = mutex.lock().unwrap_or_else(|p| { log::error!("mutex poisoned: {ctx}"); p.into_inner() });
```
Or migrate to `parking_lot::Mutex` which never poisons (already used by `CircuitBreaker`).

**Complexity:** large (sweep ~104 sites). Split by crate: `pcloud-fs`, `pcloud-daemon`, `pcloud-ipc`.

### P1-3. Panic!/expect reachable in daemon hot paths
**Consensus.** Opus §9-10 HIGH-2/HIGH-3 + Sonnet §9-10 C9-001/C9-002/C9-003/C9-007.

- `crates/pcloud-daemon/src/serve.rs:591` — `panic!("serve loop did not exit within 5s of external flag flip")`. Reachable during shutdown. **Fix:** emit `log::error!` + forced abort, not panic.
- `crates/pcloud-daemon/src/serve.rs:558` — `.expect("socket should bind")`. Bind failure must return a structured error to the supervisor. **Fix:** propagate `std::io::Error` up.
- `crates/pcloud-daemon/src/dispatch.rs:537` and `serve.rs:535` — `bootstrap_with_config(...).expect("runtime bootstrap should succeed")`. **Fix:** return `Result<_, DaemonError>` with user-facing diagnosis.
- `crates/pcloud-fs/src/fuse_adapter.rs:1389` — `.expect("just-inserted")` in `open()` hot path. **Fix:** use `Entry::or_insert_with` or return `EIO`/`ENOENT`.
- `crates/pcloud-resilience/src/retry.rs:400,403,406,409,412,459` — six bare `panic!()` in retry state machine. **Fix:** `Err(RetryError::InternalState(...))` and propagate.

**Complexity:** medium. Each site is a local fix; coordinate only for the serve-loop shutdown (needs to integrate with drain-state).

### P1-4. Sync engine durability + correctness
**Opus §4 HIGH-1..H5.** (Sonnet §4 confirms audit-04 fixes held but no new CRITICALs.)

- **H1 `resolve_newest_wins` reads from CWD-relative path** (`crates/pcloud-engine/src/conflict_resolver.rs:210-228`): `std::fs::metadata(path)` with a sync-root-relative string. **Fix:** pass absolute path or require `local_mtime_secs: Option<u64>` from the caller; error if `None` under `NewestWins`. Also pipe `remote_modified_secs` through `ConflictKind` (M-8).
- **H2 In-flight transfers not persisted** (`crates/pcloud-daemon/src/sync_loop_runtime.rs:511-533`): `Scheduler::next_batch` drains queue, leaves dispatched ops only in memory. A crash between drain and completion loses the work. **Fix:** persist `active_uploads`/`active_downloads` alongside the scheduler queue; drain+checkpoint atomically.
- **H3 Connection-global `synchronous=FULL` pragma leak** (`sync_loop_runtime.rs:1086-1115`): if `commit_diff_batch` errors between the FULL/NORMAL pragmas, the connection stays FULL for the rest of the daemon's life. **Fix:** RAII guard struct with `Drop` that restores NORMAL regardless of error path.
- **H4 StallDetector only marks progress on dispatch, not completion** (`sync_loop_runtime.rs:519-533`): long-running upload triggers false stall. **Fix:** call `stall_detector.mark_progress()` from `mark_transfer_completed` success path and from download/upload success branches.
- **H5 Cold-cache nested-upload terminal drop** (`sync_loop_runtime.rs:653-674, 1167-1184`): audit-04 C2 only half-fixed. Cold metadata cache returns `None` → `InvalidPath` → `Terminal` → item dropped forever. **Fix:** stage folder-create operations first and thread returned `RemoteFolderId` into dependent uploads; or route cold-cache to `RetryableNetworkError`.

**Complexity:** large; assign one agent to own the entire sync engine crate.

### P1-5. FUSE journal silent data loss under backpressure
**Sonnet §5 H-4.** Opus §5 does not explicitly flag the silent-eviction case but flags `max_staging_bytes` aggregate cap gap (§5 M-4) as the analog. Direct source inspection — `crates/pcloud-fs/src/journal.rs:50-54` — confirms `WritebackJournal::append` silently evicts the oldest entry at capacity (4096). **Data loss with no error returned.** Elevated to P1 by structural severity (data integrity), despite Opus rating MEDIUM on the related staging-ceiling issue.

- **File:** `crates/pcloud-fs/src/journal.rs:50-54`.
- **Fix:** return `Err(JournalError::AtCapacity)` when `pending.len() == max_pending_operations`. Callers must flush or fail the write.
- **Complexity:** small; callers must be updated to propagate.

Paired fix (Opus §5 M-4): add an aggregate `total_staging_bytes` ceiling across all inodes to prevent disk-fill attacks.

---

## Priority 2 — HIGH severity (Opus-confirmed)

### P2-1. FUSE correctness fixes
- **P2-1a. `FileHandle.size=0` breaks stat/mmap** (§5 disagreement resolved HIGH above; §5-opus L-3 + §5-sonnet C-2) — `crates/pcloud-fs/src/backend.rs:267-275`. **Fix:** call `getfileinfo` after `getfilelink`; populate `size` before returning. Remove the log::warn! spam. **Complexity:** small-medium.
- **P2-1b. `eprintln!` in production read path** (§5-sonnet C-1, direct source verification at `backend.rs:304-310` confirms it). **Fix:** replace with `log::trace!`. **Complexity:** small.
- **P2-1c. BSD + Windows signal-driven mount cleanup absent** (§5-opus M-1). `platform/bsd.rs` + `platform/windows.rs` register no SIGTERM/SCM STOP reaper. **Fix:** parallel `sigaction` reaper for BSD (mirror `linux.rs:659-722`); `SetConsoleCtrlHandler` + SCM stop callback for WinFSP. **Complexity:** medium.
- **P2-1d. `ACTIVE_MOUNTS` canonicalization race** (§5-opus M-2, `linux.rs:630-646`). **Fix:** capture canonical `PathBuf` once in `LinuxMountHandle`, reuse on register/unregister.
- **P2-1e. Reaper silent `.ok()` spawn** (§5-opus M-3, `linux.rs:690-696`). **Fix:** panic in debug, `log::error!` in release, refuse mount if spawn fails.
- **P2-1f. Aggregate staging ceiling absent** (§5-opus M-4, `write_path.rs:291-313`). **Fix:** `total_staging_bytes` computed from `StagingDir` size; fail new `create_blob` with `ENOSPC` at ceiling.
- **P2-1g. Chunked `upload_write` pipelining still absent** (§5-opus M-5). Blocks multi-GiB write claims for `bd-1du.10`. **Fix:** implement `upload_chunk_begin/write/finish` in `PcloudFsBackend` and wire flush loop.
- **P2-1h. `invalidate_file` O(n) holds global cache mutex** (§5-sonnet H-5, `page_cache.rs:293-306`). **Fix:** add secondary index `file_id → Vec<PageKey>`; reduce to O(k). (Sonnet-only; direct inspection confirms the loop walks the entire LRU. Opus did not flag because the overall LRU fix landed — but the per-file invalidate didn't benefit. Elevated after code inspection.)

### P2-2. Crypto hardening (§3-opus C-1 + H-1..H-3; Sonnet adds evidence)
- **P2-2a. `BackendMismatch` variant unreachable** (§3-opus H-1, `lib.rs:306` vs construction sites). **Fix:** emit `BackendMismatch { expected, provided }` from `change_password_with_context` (`lib.rs:1992`), from legacy `seal_sector`/`open_sector` fallbacks (`lib.rs:2458,2565`), and from the dispatch bailouts.
- **P2-2b. Sentinel-inferred backend silent migration** (§3-opus H-2, `lib.rs:1348-1368`). **Fix:** `effective_backend` at `:748-759` must combine `setup_fingerprint.is_some()` **and** `pclsync_compat.is_some()` to classify correctly; refuse ambiguous state.
- **P2-2c. Pclsync CTR LE not wire-compatible on BE hosts** (§3-opus H-3, `pclsync_modes.rs:93-101,117`). **Fix:** add `const _: () = assert!(cfg!(target_endian = "little"))` compile-time guard; rename module docstring to "LE-only (x86_64/aarch64)".
- **P2-2d. PBKDF2 5000-iter legacy path may be default** (§3-sonnet HIGH-2, `password_scorer.rs:540-681`). **Opus §3 did not re-flag in audit-05 (was audit-04 P2-2 H-3). Still not landed — re-verify.** **Fix:** confirm `legacy-c-compat` is NOT in default features; if it is, remove; document migration.
- **P2-2e. `sectors_sealed` counter is `#[serde(skip)]`** (§3-sonnet HIGH-1, `lib.rs:678`). Nonce-budget resets on daemon restart. **Fix:** either persist `sectors_sealed` or rotate per-file seeds per process lifetime. (Sonnet-only; Opus §3 M-4 flagged the atomic ordering but not the reset. Direct inspection of `#[serde(skip)]` attribute confirms Sonnet's finding.)
- **P2-2f. RSA-4096 signing for share_temppass (rows 124, 142)** (§3-opus §1-opus H-02 + §3-sonnet HIGH-3). Tracked `bd-1du.5`. Blocks closure of two Partial rows. **Fix:** land RSA-4096 keypair + `prsa_sign_sha256_hash` equivalent in `share_temppass.rs`.

### P2-3. Transport (§6-opus H-1..H-4)
- **P2-3a. TLS doc-vs-code drift** (§6-opus H-1, `tls.rs:12` vs `:49-53`). Code is TLS1.3-only (correct); doc says 1.3+1.2 (wrong). **Fix:** update rustdoc to "TLS 1.3 only"; add test rejecting TLS1.2 suites.
- **P2-3b. `classify_error` string-matching** (§6-opus H-2 + §6-sonnet M-4, `pcloud-resilience/src/transport.rs:227-240`). **Fix:** typed error classification (mirror the binary stack's `transport_error_classifier`).
- **P2-3c. Observability TODOs unlanded on binary path** (§6-opus H-3 + §6-sonnet H-6.1 + §9-10-sonnet C9-009, `resilient_transport.rs:302-305,356-365`). **Fix:** wire `pcloud-observability` into `execute`; emit `pcloud_transport_latency_seconds` and `pcloud_transport_errors_total`.
- **P2-3d. Upload idempotency string-match** (§6-opus H-4, `resilient_transport.rs:416-421`). Only matches `"upload_write"` / `"upload_save"` — misses `upload_writefromfile` and future variants. **Fix:** `MethodClass::Idempotent | Mutation` tag on `EncodedRequest`; exhaustive enum forces new variants to classify.

### P2-4. IPC/daemon HIGH (§7-opus H1)
- **P2-4a. Privileged audit logs daemon uid not peer uid** (§7-opus H1, `serve.rs:204-211`). **Fix:** thread `PeerIdentity.uid` from `transport.rs:319,502` through the handler closure; log `peer_uid` and `peer_pid`.

### P2-5. Security hardening (§2-opus MEDIUM elevated to P2 consensus)
- **P2-5a. IPC parent-dir mode not tightened on pre-existing dirs** (Consensus: Opus §2 MED-2.3 + Sonnet §2 M-4 + Sonnet §2 M-2). `crates/pcloud-ipc/src/transport.rs:621-642`. **Fix:** unconditionally `set_permissions(parent, 0o700)` regardless of `parent_missing`.
- **P2-5b. Auth vault parent-dir mode unchecked on load** (Sonnet §2 M-2, direct inspection of `vault/file.rs:215-237` confirms). **Fix:** add parent-dir mode check in `validate_vault_file`.
- **P2-5c. KAT extractor plaintext-password auth** (Consensus: Opus §2 MED-2.4 + Sonnet §2 H-2). `scripts/extract-pclsync-kat.py:109-115,213-215`. **Fix:** digest-only (`getdigest` first, always `passworddigest`); remove plaintext-first fallback; document env-var hygiene in script header.

### P2-6. CI/Testing coverage gaps (§9-10 consensus HIGH)
- **P2-6a. FreeBSD excludes `pcloud-fs` with `continue-on-error: true`** (§9-10-sonnet C10-003). **Fix:** add a mock-backend compile test for `pcloud-fs` on FreeBSD; land BSD adapter per P2-1c.
- **P2-6b. macOS CI excludes pcloud-fs integration tests** (§9-10-sonnet C10-004). **Fix:** document gap explicitly in STATUS.md under "CI coverage matrix"; open bead for macOS self-hosted runner.
- **P2-6c. Cross-platform CI matrix verification** (§9-10-opus HIGH-5). **Action:** inspect `ci.yml` platform matrix; verify Linux/macOS/Windows/FreeBSD all run non-mount test suites; gate release on green.
- **P2-6d. `unsafe` blocks missing `// SAFETY:`** (§9-10-opus HIGH-4, delta 46 = 364 unsafe vs 318 SAFETY). **Fix:** sweep; concentration in `pcloud-fs/src/platform/*.rs`. Add missing SAFETY comments; `pcloud-compat/src/folder_list.rs:214,225,250,267` shm pointers cited by Sonnet §9-10 C9-005.

### P2-7. Deployment/packaging
- **P2-7a. Systemd FUSE drop-in example** (Sonnet §11-12 H-2, confirmed analog of audit-04 IPAddressAllow override pattern). `packaging/systemd/pcloudd.service:49,89` blocks FUSE; no `fuse-override.conf.example`. **Fix:** ship the drop-in with `PrivateDevices=no`, `SystemCallFilter=@mount`, `ReadWritePaths=/dev/fuse /run/user/%U`. **(Sonnet-only per H-2 but cross-validated by the audit-04 override.conf pattern — prioritized.)**
- **P2-7b. FreeBSD rc.d passes unsupported `-p` flag** (Opus §11-12 M-2). `packaging/freebsd/pcloudd.rc:50-52`. **Fix:** drop `command_args`; use `daemon(8)` or wire real pidfile option.
- **P2-7c. macOS launchd plist advertises ignored env vars** (Consensus: Opus §11-12 M-3 + Sonnet §11-12 M-4). `packaging/macos/com.pcloud.pcloudd.plist:97-106`. **Fix:** remove the 5 compat-alias keys; keep only the vars the daemon reads.
- **P2-7d. launchd `com.pcloud.pcloudd.plist` missing `ExitTimeOut`** (Sonnet §11-12 M-2). **Fix:** add `<key>ExitTimeOut</key><integer>30</integer>`; dedupe with `com.pcloud.pcloud-rs.plist` (one is redundant).
- **P2-7e. `API-REFERENCE.md` Partial catalogue incomplete** (Consensus: Opus §11-12 M-1 + Sonnet §11-12 M-3). Only lists row 93; misses 26, 27, 124, 142. **Fix:** add Partial rows for TFA (under Auth) and crypto-share (under Shares), each citing `bd-1du.5` / symmetric-HMAC-vs-RSA-4096 rationale.
- **P2-7f. `CRYPTO-BACKEND-PLAN.md` header stale** (Sonnet §11-12 M-1). `docs/CRYPTO-BACKEND-PLAN.md:3` says "Planning. No code changes yet" but Wave 2 is shipped. **Fix:** update `Status:` to "Implemented (Wave 2 shipped 2026-04-18)".
- **P2-7g. Release cosign unwired** (Opus §11-12 M-4). **Fix:** enable keyless signing (`id-token: write` in release.yml:99) or document "releases are currently unsigned" in CONTRIBUTING.md.
- **P2-7h. Security workflow anemic** (Opus §11-12 M-5). **Fix:** extend `security.yml` with `cargo deny check` + `grype` on SBOM; upload SARIF.
- **P2-7i. `nfpm.yaml` binary name mismatch** (Opus §11-12 L-3). `pcloud-rs` vs `pcloudc`. **Fix:** verify actual binary name; align packaging.

---

## Priority 3 — MEDIUM (Opus-confirmed, batched by crate)

Grouped batches so single agents own a coherent surface:

### P3-batch-A — Crypto MEDIUM
- **§3-opus M-1** RSA-OAEP fallback parser padding-oracle shape: gate `normalize_candidates` as `#[cfg(test)]`-only (`pclsync_compat_kat_live.rs:80-128`).
- **§3-opus M-2** Short-plaintext (`datalen==0`) sector silently supported: add `SectorError::EmptyPlaintext` or document as extension (`pclsync_sector.rs:364-380`).
- **§3-opus M-3** `pclsync_auth_tree` ships pure-HMAC half only, not byte-identical to C (disclosed at `pclsync_auth_tree.rs:36-47`). Fix STATUS.md wording or land the AES wrap.
- **§3-opus M-4** Brute-force lockout counters `Relaxed`-ordered; racing attempts can shorten backoff. Use `Mutex<LockoutState>` or `AcqRel` CAS.
- **§3-sonnet MED-1** `cache_ttl_secs` auto-stop timer dead (`keys.rs:58-68`). Wire or remove.

### P3-batch-B — Sync engine MEDIUM (Opus §4 M1-M8)
- M1 scheduler fairness cap round-robin cursor across cycles.
- M2 transfer-coordinator eviction on pause persists nothing; pair with H2 fix.
- M3 `FsEventIngestor` cross-batch debouncing (500ms coalesce).
- M4 bandwidth scheduling absent; wire `pcloud-config/src/rate_limit.rs` into sync_loop_runtime.
- M5 `emit_cycle_audit` skips idle cycles; emit heartbeat every N cycles.
- M6 `resolve_upload_payload_len`/`borrow_upload_payload` TOCTOU; pin buffer or re-validate len.
- M7 `walk_recursive` inode cycle check ignores `dev_t`; use `(st_dev, st_ino)`.
- M8 `ConflictResolver::resolve_conflicts` always passes `None, None`; plumb remote mtime.

### P3-batch-C — FUSE MEDIUM
- **§5-sonnet M-2** 24-hour flush interval default disables auto-flush (`write_path.rs:311`). Drop to 30-60s.
- **§5-sonnet M-3** `log::warn!` on every open in hot path (`backend.rs:272`). Demote to `trace!` (superseded by P2-1a).
- **§5-sonnet M-4** macOS teardown `drop(joiner)` UAF risk (`mount_service.rs:551-559`). Mirror Linux bounded-join with forced abort.
- **§5-sonnet M-1** `fuser_shim.rs` Linux-only idiom without cfg gate (lines 17, 25). Add `#[cfg(target_os = "linux")]`.

### P3-batch-D — Transport MEDIUM (Opus §6 M1-M6 + Sonnet M-6.1..M-6.3)
- M1 two divergent resilience stacks (proto sync vs resilience async). Collapse or document.
- M2 `Retry-After` cap inconsistent across 3 sites. Align on single parser.
- M3 breaker state not metriced. Add gauge/callback.
- M4 connect_socket lacks happy-eyeballs. RFC 8305 fallback.
- M5 `total_request_timeout` not propagated to `ResilientTransport`. Share deadline.
- M6 BandwidthPacer uses `std::sync::Mutex` (poisons); migrate to `parking_lot`.
- Sonnet M-6.1 `is_known_safe_host` duplicated with divergent semantics. Unify.
- Sonnet M-6.2 BandwidthPacer not wired to HTTP download path. Verify + add integration test.

### P3-batch-E — IPC/daemon MEDIUM
- **§7-opus M1** "Per-session" rate limiter is process-global. Fix: `DashMap<peer.pid, SessionRateLimiter>`; or rewrite docs honestly.
- **§7-opus M2 + §7-sonnet M-1** `CryptoGetFolderKey` / `CryptoGetFileKey` not in `is_privileged_request` (Consensus). **Fix:** add both variants to the `matches!` block at `serve.rs:~89`.

### P3-batch-F — CLI/SDK MEDIUM (Opus §8 M1-M3 + Sonnet §8)
- Opus M1 `crypto get-folder-key`/`get-file-key` missing output-redaction contract. Add `--show-cache-only` or `PCLOUD_DEBUG_KEY_MATERIAL=1` gate.
- Opus M2 `--allow-argv-password` cannot scrub `/proc/self/cmdline`. Document residual in OPERATIONS-RUNBOOK.md.
- Opus M3 Completion tree incomplete for Wave 2 flags (Consensus with Sonnet §8 "Completion tree is a strict subset"). Add positional `<FOLDER_ID>`/`<FILE_ID>`; surface `--password-stdin`/`--password-env`/`--allow-argv-password` at root; add sync `change-type`/`localscan`/`suggest`/`is-syncable`/`status` entries.
- Sonnet §8 workspace `repository`/`homepage` URL points to upstream (`Cargo.toml:63-64`). Update to `github.com/ezechiel203/pcloud-rs`.
- Sonnet §8 `unsafe std::env::remove_var` in `main.rs:2070-2072`. Document invariant with `static_assertions` or migrate to safer idiom.

### P3-batch-G — Testing/quality MEDIUM
- **§9-10-opus M6** New pclsync modules have `unwrap()` leakage outside `#[cfg(test)]`. Per-site classify and sweep.
- **§9-10-opus M7** 6 TODO markers without bead-ID. Link or convert to prose.
- **§9-10-opus M8** `Drop` impls sparse (28) vs RAII surface. Add `Drop` on `AuthVault`, IPC listener, journal writer.
- **§9-10-opus M9** 105 `#[ignore]` tests need documented justifications in CONTRIBUTING.md.
- **§9-10-opus M10** Fuzz skewed to proto. Add `fuzz_auth_vault_decode`, `fuzz_crypto_filename_decode`.
- **§9-10-opus M11** Missing crypto sector bench. Add to `pcloud-crypto/benches/`.
- **§9-10-opus M12** 28 `.ok();` error-drops; review each for silent-failure rule.
- **§9-10-sonnet C9-006** `unsafe set_var` in test code races. Use global `Mutex<()>` or per-process env injection.
- **§9-10-sonnet C10-005** Fuzz target `.rs` files may be absent. Verify commit.
- **§9-10-sonnet C10-006** `proptest_methods_roundtrip.rs` coverage unverified. Add `#[derive(Arbitrary)]` on all `Request` variants.

### P3-batch-H — Deployment/docs MEDIUM
- Sonnet §11-12 M-5 No Prometheus dashboard/alert rules. Ship `dashboards/` with Grafana JSON + scrape config; document `/metrics` in OPERATIONS-RUNBOOK.md.
- Sonnet §11-12 L-3 `cargo doc` gate absent from CI. Add `cargo doc --workspace --no-deps 2>&1 | grep "^warning" && exit 1`.
- Sonnet §11-12 L-2 CHANGELOG no release-gate CI check.

---

## Priority 4 — LOW (Opus-confirmed)

### §1 Parity
- **§1-opus L-1** Rejected rationales not hyperlinked. Add `(see REJECTED-RATIONALES-14042026.md#row-N)` to each matrix note.
- **§1-opus L-2** KAT README provenance incomplete. Add signed `provenance.txt` next to fixtures.

### §2 Security
- **§2-opus LOW-2.5** `--acknowledge-not-interop` IPC-field single-bool, no replay protection. Add daemon-side setup nonce.
- **§2-opus LOW-2.6** `auth_vault.rs` shim pointer — confirm `vault/file.rs` still enforces 0600/0700/ownership on next pass.

### §3 Crypto
- **§3-opus L-1..L-4** Salt length not statically asserted (L-1 — already safe), `SymKeyVer1` aggregate has non-secret fields (L-2, harmless), formatting (L-3), `plaintext.to_vec()` non-zeroizing recovery path (L-4 → `Zeroizing<Vec<u8>>`).
- **§3-sonnet LOW-1** `NONCE_BUDGET_SAFETY_MARGIN=64` is tight. Raise or track per-file-key.

### §4 Sync engine
- **§4-opus L1-L8** SHA-256 8-byte staged-path collision (L1), `ReconcileWorker::interval` 300s cold-start (L2), `ValuesRepository::get_string` silent skip (L3), `SCHEDULER_QUEUE_KEY` full-queue-JSON-per-tick (L4), `list_sync_roots` single-read evicts watchers (L5), `spawn_sync_loop` panics on spawn (L6), `evict_sync_root` leaves overflow (L7), `FsWatcher::start` fallback warn-only (L8).

### §5 FUSE
- **§5-opus L-1..L-4** Journal parent-dir fsync `let _ =` (L-1), `fuser_shim.rs` cfg gate note (L-2, dedup with P3-batch-C M-1), `backend.rs:268` size=0 TODO (superseded by P2-1a), `unsafe impl Send/Sync` SAFETY comment thinness on WinFSP vtable (L-4).

### §6 Transport
- **§6-opus L-1..L-6** `is_retryable_io` over-broad (L-1), `ResponseTooLarge` hardcoded (L-2), `MAX_RESPONSE_BYTES=64 MiB` eager alloc (L-3), `fetch_download_resumable` ignores retry_after (L-4), no keep-alive (L-5), `default_classifier` marks `InvalidInput` transient (L-6).

### §7 IPC/daemon
- **§7-opus L1-L6** dead dispatch arms for Partial rows (L1), sd_notify Linux-only (L2), health endpoint thread-per-connection (L3), per-peer cap degenerate under owner-only (L4), socket cleanup on SIGKILL race (L5), Wave-2 daemon-proptest gate missing (L6).

### §8 CLI/SDK
- **§8-opus L1-L4** Help text missing Wave 2 subcommand descriptions (L1), SDK structs not `#[non_exhaustive]` (L2), no SDK examples for new flows (L3), `commands.rs:807` stray single-slash doc comment (L4).

### §9-10 Quality/testing
- **§9-10-opus L13-L16** Unwrap-count bead per top-5 crate (L13), workspace clippy `unwrap_used`/`expect_used` deny in non-test (L14), KAT doc-format adoption across live tests (L15), security.yml/fuzz.yml verification (L16).

### §11-12 Deployment/docs
- **§11-12-opus L-1** `CONTRIBUTING.md:159,228` broken `../CLAUDE.md` link; should be `./CLAUDE.md`.
- **§11-12-opus L-2** NFC caveat not echoed in KAT runbook.
- **§11-12-opus L-4** SUMMARY.md cosmetic indentation.

---

## Consensus tier (≥2 auditors agreed)

These are prioritized **within** their severity bucket. If multiple agents/auditors hit the same finding independently, confidence is high and these land first in each wave.

| Finding | Priority | Opus ref | Sonnet ref |
|---|---|---|---|
| STATUS.md three contradictory headlines | P0-1 | §1 C-01, §11-12 H-2 | §1 authoritative parse, §11-12 H-3 |
| CLAUDE.md stale 156/2/0/28 | P0-2 | §11-12 H-1 | §11-12 H-1 |
| Row 26 `psync_tfa_has_devices` zero code | noted (P0 via STATUS) | §1 H-01 | §1 HIGH row 23 |
| Row 27 `psync_tfa_type` zero code | noted (P0 via STATUS) | §1 H-01 | §1 HIGH row 24 |
| Row 93 upload_writefromfile stub | P0-3 | §1 C-02 | §1 HIGH row 93 |
| Rows 124/142 RSA-4096 signing | P2-2f | §1 H-02, §3 | §1 MED, §3 HIGH-3 |
| `PclsyncCompatProfile` Debug redaction | P1-1a | §2 HIGH-2.1 | §2 M-3 |
| IPC parent-dir mode not tightened | P2-5a | §2 MED-2.3 | §2 M-4 |
| KAT extractor plaintext-password | P2-5c | §2 MED-2.4 | §2 H-2 |
| KAT `#[ignore]` + env-gated (CI gap) | P0-4 | §1 H-03 | §3 C3-SON-LOW-2, §9-10 C10-001 |
| Mutex poison unwrap sweep | P1-2 | §9-10 HIGH-1 | §9-10 C9-004 |
| Panic!/expect in daemon bootstrap | P1-3 | §9-10 HIGH-2,3 | §9-10 C9-001,2,3,7 |
| Observability TODO on binary path | P2-3c | §6 H-3 | §6 H-6.1, §9-10 C9-009 |
| Classify_error string-match | P2-3b | §6 H-2 | §6 M-4 |
| Privileged log: CryptoGetFolderKey/FileKey | P3-batch-E | §7 M2 | §7 M-1 |
| API-REFERENCE.md Partial catalogue incomplete | P2-7e | §11-12 M-1 | §11-12 M-3 |
| launchd plist ignored env vars | P2-7c | §11-12 M-3 | §11-12 M-4 |
| Completion tree incomplete | P3-batch-F | §8 M3 | §8 M-completion |

---

## Needs Opus Validation (Sonnet-only; do NOT act until validated)

Park these until a targeted Opus validator confirms by direct source inspection. If confirmed, promote to the corresponding priority bucket.

1. **§5-sonnet H-5 `invalidate_file` O(n) holds global cache mutex** (`page_cache.rs:293-306`). Included as P2-1h above since direct source inspection confirms the O(n) scan; re-validate severity with Opus.
2. **§9-10-sonnet C9-005 `unsafe` without SAFETY in `pcloud-compat/src/folder_list.rs:214,225,250,267`** (shm pointers). Direct confirmation needed.
3. **§9-10-sonnet C10-002 "no bench files exist"** — contradicts Opus §9-10 count of 13 bench targets. Validator must reconcile: likely Sonnet globbed incorrectly; Opus list is authoritative. Downgrade or drop if refuted.
4. **§2-sonnet H-1 PBKDF2-20k iteration warning** — wire-locked constant, but document.
5. **§2-sonnet L-3 `CryptoPolicy::auto_lock_idle_secs=0` default disables auto-lock.** Validate intent; if unintentional, raise to MEDIUM.
6. **§4-sonnet M-4 `planner_overflow` unbounded memory under diff-flood.** Add `max_overflow_depth: usize` cap.
7. **§11-12-sonnet H-2 systemd FUSE drop-in absent** — promoted to P2-7a above, but Opus validator should confirm the syscall-filter + ReadWritePaths claim matches running behavior.

**Proposed validator agent prompt:** "Verify each of these 7 Sonnet claims by opening the cited file at the cited line range. Report confirmed/refuted/partial with quoted evidence. 300 words max."

---

## Execution Order

| Wave | Priority | Work | Est. Agents |
|------|----------|------|-------------|
| 1 | P0-1, P0-2 | Parity truth: STATUS.md + CLAUDE.md headline scrub | 1 |
| 1 | P0-3 | Row 93 IPC variant rewrite (Option A) or flip to Missing (Option B) | 1 |
| 1 | P0-4 | Offline KAT variant with committed fixtures | 1 |
| 1 | — | Opus validator on 7 Sonnet-only items | 1 Opus validator |
| 2 | P1-1 to P1-5 | CRITICAL code defects (secret-Clone/Debug, mutex poison, panic sweep, sync engine, FUSE journal) | 5 parallel |
| 3 | P2-1 to P2-7 | HIGH severity: FUSE correctness, crypto hardening, transport, IPC, security, CI, packaging | 7 parallel |
| 4 | P3 batches A-H | MEDIUM sweep | 4 parallel (batches grouped) |
| 5 | P4 | LOW polish | 2 parallel |

**Parallelization notes:**
- Waves 2 and 3 must not touch the same crate in parallel — assign FUSE agent exclusively to `pcloud-fs`; sync-engine agent exclusively to `pcloud-engine` + `pcloud-daemon/sync_loop_runtime.rs`; etc.
- Wave 1 P0-3 (row 93) must complete before any audit-06 re-run because STATUS.md truth depends on the chosen option (A keeps 5 Partial, B flips row 93 to Missing → 4 Partial / 1 Missing).
- Wave 2 P1-2 (mutex poison sweep) touches files across daemon/fs/ipc — split by crate to avoid merge conflict.

---

## Gate criteria

For audit-06 to see this fix wave as **closed**, all of the following must be true:

**Wave 1 gate (parity honesty):**
- `STATUS.md` contains exactly one current headline (`153 / 5 / 0 / 28` or whatever Row 93 settles at under P0-3).
- `CLAUDE.md` contains zero references to `156 / 2 / 0 / 28` or `155 / 3 / 0 / 28` outside a dated, fenced superseded-history block.
- `C_FEATURE_PARITY_MATRIX.csv`, `STATUS.md`, `CLAUDE.md`, `API-REFERENCE.md` all agree on the Partial row list.
- Row 93: either IPC rewired with live-e2e test passing, or matrix flipped to Missing with rationale in `REJECTED-RATIONALES-*.md`.
- Offline KAT variant runs green under `cargo test` on every CI job.

**Wave 2 gate (CRITICAL code defects):**
- Zero `Clone` derives on types wrapping raw key material (grep confirms).
- `cargo test --workspace` green on Linux, macOS, Windows, FreeBSD.
- No `.unwrap()` / `.expect()` on `Mutex::lock()` in `pcloud-fs`, `pcloud-daemon`, `pcloud-ipc` src trees (excluding `#[cfg(test)]` modules).
- Zero reachable `panic!` / `unreachable!` in daemon serve loop, FUSE handlers, retry state machine.
- Sync engine: `H3` pragma RAII guard landed; `H1` NewestWins uses absolute path; `H2` in-flight transfers persisted.
- FUSE journal: `append` returns `Err(AtCapacity)` not silent evict.

**Wave 3 gate (HIGH):**
- No HIGH from Opus audit open.
- `FileHandle.size` populated from `getfileinfo` before return; `stat(2)` on a mounted file returns correct size.
- `BackendMismatch` variant constructed from at least 3 dispatch sites.
- systemd FUSE drop-in shipped; launchd plist ignored-env-vars removed; FreeBSD rc.d fixed.
- `API-REFERENCE.md` lists all 5 Partial rows.
- Observability metrics emitted on binary transport path.

**Wave 4 gate (MEDIUM):**
- Audit-06 re-run with 10+10 agents shows no regression from audit-05 severity bands.

**bd-1du.10 closure criteria (carries forward from audit-04):**
All Wave 1-4 gates above, plus:
- Cross-platform mount hardware verification (macOS fuse-t, Windows WinFSP, BSD fusefs) — human task, out of AI scope.
- Human reviewer sign-off on parity matrix — out of AI scope.
- Reproducible-build CI passing with bit-identical digests across two runners.
- No doc claims "production ready" / "full parity" / "enterprise ready" / "drop-in replacement" (CLAUDE.md rule).

---

## Word count appendix

This plan: ~3,800 words across 7 priority bands, 20 reports synthesized, 60+ distinct actionable items, 18 consensus findings, 7 Sonnet-only items parked for validation, 5-wave execution order with gate criteria.

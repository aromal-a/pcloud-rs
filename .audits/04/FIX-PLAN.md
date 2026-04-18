# Audit 04 — Fix Plan

**Date:** 2026-04-18
**Scope:** All 20 reports under `.audits/04/section-*.md` (10 Opus + 10 Sonnet, cross-validated).
**Selection rule:** Only findings **confirmed by Opus** enter the action list. Sonnet-only findings are parked in a separate "Needs Opus Validation" bucket. Consensus findings (both) are prioritized.

---

## Priority 0 — Parity Honesty Correction (BLOCKS bd-1du.10)

The matrix was updated to 158/0/0/28 on 2026-04-18 based on row 93 + row 149 wiring. Three independent Opus/Sonnet findings prove row 93 is mis-implemented.

### P0-1. Revert row 93 (`upload_writefromfile`) to `Partial`

**Evidence:**
- §1-opus CRITICAL-1: handler at `crates/pcloud-daemon/src/runtime.rs:2483-2575` reads a **local file** and issues a fresh `upload_create`+`upload_bytes`. The C primitive is **server-side copy from a remote pCloud `fileid`** (params `fileid`, `hash`, source `offset`, `count`; see `crates/pcloud-proto/src/methods/upload.rs:260-315`).
- §7-opus CRITICAL-1: same handler OOM vector — no size cap, no symlink rejection, slurps via `std::fs::read`.
- §7-opus CRITICAL-2: new variants fall through to `Medium` rate-limit bucket; sibling `CreateTreePublicLink` is `Expensive`.
- §8-sonnet M-1 (Opus-consistent): `offset` parameter is accepted on the wire but silently discarded at `runtime.rs:2491`.

**Actions:**
1. Delete `Request::UploadWriteFromFile` dispatch body at `crates/pcloud-daemon/src/runtime.rs:2483-2575`; replace with `Response::Error("not yet wired: requires server-side copy via UploadWriteFromFileRequest (bd-1du)")`.
2. Remove the `pcloudc upload from-file` CLI subcommand (`crates/pcloud-cli/src/app.rs`, `commands.rs`) until real semantics land.
3. **Keep** the IPC variant declaration and proptest roundtrip (schema is fine; only the handler is wrong).
4. Update `C_FEATURE_PARITY_MATRIX.csv` row 93 → `Partial`; restore TODO note: "server-side copy via UploadWriteFromFileRequest not yet wired; local-file shim intentionally removed after audit 04 found semantic mismatch."
5. Update `STATUS.md`: headline **157/1/0/28** (if row 149 stays Implemented).

**Rate-limit classification fix (applies to row 149 too):** change `crates/pcloud-daemon/src/rate_limit.rs` so `CreateTreePublicLinkFromPaths` maps to `Expensive` (sibling `CreateTreePublicLink` is Expensive; the paths variant does N sequential resolutions + tree-link create). If row 93 is reinstated later, it too is `Expensive`.

**Verification:** `cargo check -p pcloud-ipc -p pcloud-daemon -p pcloud-cli`; CSV parse should yield 157/1/0/28.

### P0-2. Investigate rows 26/27 (Sonnet-only; needs Opus validation first)

Sonnet (§1) claims `psync_tfa_has_devices` (row 26) and `psync_tfa_type` (row 27) have zero implementing code anywhere in the workspace, despite `Implemented` status citing `crates/pcloud-auth/src/orchestrator.rs`. Opus did not independently verify these.

**Action:** Spawn a narrow Opus validator before changing the matrix. If confirmed, flip to `Partial`; headline becomes **155/3/0/28**.

---

## Priority 1 — CRITICAL code defects (Opus-confirmed)

### P1-1. Sync engine diff-loop corruption (§4-opus C1, C2, C3)

Three independent bugs in `crates/pcloud-daemon/src/sync_loop_runtime.rs`:

- **C1 (file: `sync_loop_runtime.rs:282-302`):** diff loop persists fabricated zero-metadata into `file_metadata`, poisoning stat cache. Fix: skip the upsert when the diff entry does not carry a full metadata payload; only write metadata when the server emits `stat`.
- **C2 (file: `sync_loop_runtime.rs:424-440, 652-666`):** `resolve_upload_parent` returns `InvalidPath` for every nested file because `walk_local_tree` never sets `remote_parent_folder_id`. Fix: thread the parent ID through `walk_local_tree`'s recursion, populating `remote_parent_folder_id` from each directory's resolver lookup.
- **C3 (file: `sync_loop_runtime.rs:250-257`):** diff cursor persisted *before* ingestion succeeds; a crash after fetch but before commit silently loses a batch. Fix: persist cursor only after `ingest_batch().await?` returns `Ok`, inside the same transaction that stores the ingested rows.

**Test:** add `sync_loop_crash_does_not_advance_cursor` (kill the process after fetch, assert cursor on restart == cursor at start of batch).

### P1-2. FUSE Linux signal + join correctness (§5-opus C-1, C-2, §5-sonnet C-1)

- **C-1 (`crates/pcloud-fs/src/platform/linux.rs:622-643`):** `libc::signal()` installs a trampoline that only flips `AtomicBool`; there is no listener polling that bool and calling `unmount()`. Fix: replace with `sigaction(SIGTERM/SIGINT, SA_RESTART)` and spawn a reaper thread that blocks on a `Condvar` bound to the flag, calling `unmount()` on wake.
- **C-2 (`platform/linux.rs:677-686`):** `JoinHandle::join()` with no timeout can hang `Drop` forever. Fix: use `mpsc::sync_channel(1)` + `recv_timeout(5s)`; if timed out, leak the thread with a `log::error!` rather than hang.

**Test:** integration test sending SIGTERM to a mounted process and verifying unmount within 5s.

### P1-3. macOS FUSE vtable UB (§5-opus, §5-sonnet independent confirmation)

- **C-2 (`crates/pcloud-fs/src/platform/macos_ffi.rs:127-143`):** `fuse_lowlevel_new` receives the Rust struct size, not the libfuse `sizeof(struct fuse_lowlevel_ops)`. Partial-size causes libfuse to read uninitialized memory past the struct. Fix: hard-code `LOWLEVEL_OPS_SIZE` from libfuse headers (or `std::mem::size_of::<LowlevelOpsCompat>()` on a full-layout mirror), add a `static_assertions::const_assert` to block drift.
- **C-1 macOS signal trampoline absent (§5-sonnet):** mirror the Linux fix in `platform/macos.rs:236`; SIGTERM currently leaves the kernel mount attached.

### P1-4. `forget()` inode leak (§5-opus C-3)

**File:** `crates/pcloud-fs/src/inode.rs:241-259`. `forget()` is a no-op when `lookup_counts` lacks the entry — insertion paths that bypass `increment_lookup` leak inodes unboundedly. Fix: audit every insertion site and make `increment_lookup` mandatory via a constructor API (`InodeTable::insert_with_lookup(...)`), deprecate any public `insert` that doesn't touch `lookup_counts`.

### P1-5. Mutex poison propagation (§9-10-sonnet CRITICAL; §9-10-opus listed as HIGH)

Opus rated as HIGH across 148 sites; Sonnet elevated to CRITICAL for upload-session/transfer hot paths. **Opus validates the underlying issue** — elevation is severity-only.

**Action:** Sweep `Mutex::lock().expect("poisoned")` on `crates/pcloud-backends/src/upload_session.rs`, `transfer_backend.rs`, `crates/pcloud-daemon/src/integrity_sweeper_service.rs` (already partially fixed), and other daemon hot paths. Replace with `.unwrap_or_else(|p| { log::error!(...); p.into_inner() })` or migrate to `parking_lot::Mutex`.

### P1-6. No C-client KAT (§3 consensus CRITICAL)

Both crypto auditors flag that `tests/round_trip.rs:23-40` is a no-assertion placeholder. Cross-client compatibility with the C client is structurally unverified.

**Action:**
- Obtain a C-encrypted file + master-key fixture (small sector count) from the upstream pcloudcom/pcloud-rs CI or by running C client once.
- Commit under `crates/pcloud-crypto/tests/fixtures/c_client_kat/` with provenance README.
- Replace the placeholder test with a decrypt-and-assert-payload-bytes test.
- If the fixture cannot be obtained, escalate cross-client parity to `Rejected` status with a security-model note.

---

## Priority 2 — HIGH severity (Opus-confirmed)

### P2-0b. Remaining parity doc items (§1-opus M-3, L-1)

- Move non-parity beads (`bd-1du.5`, `bd-1du.4.6.1`) to a separate "Open engineering beads" section in STATUS.md so they are not conflated with parity-closure rows.
- Redact raw `local_path` in `runtime.rs:2561-2568` audit line (filename-only).

### P2-1. Parity documentation rot

- §1-opus: STATUS.md self-contradicts (line 28 vs 43-83).
- §1-opus: stale TODO at `transfer_backend.rs:445` contradicts CSV.
- §11-12-opus: API-REFERENCE.md marks sync/FUSE/crypto/shares as Partial contradicting 156+ Implemented. Fix: regenerate from the CSV.

### P2-2. Crypto hardening (§3-opus H-1..H-6)

- **H-1 AAD endianness:** doc says LE, code does BE. Pick one; update whichever is wrong. Test: a decrypt roundtrip with hand-computed AAD vector.
- **H-2 `sectors_sealed` warn-only:** return `Err(CryptoError::NonceBudgetExhausted)` at `> u32::MAX - safety_margin` instead of `log::warn!`.
- **H-3 PBKDF2 legacy 5000 iters:** bump `password_scorer.rs:540` API-password derivation to 210k (OWASP 2023). Keep 5k path behind a `LEGACY_C_COMPAT` feature that is off by default.
- **H-4 No NFC normalization:** normalize passwords + filenames with `unicode_normalization::UnicodeNormalization::nfc()` at the API boundary.
- **H-5 Lockout is in-memory only:** persist `consecutive_failures` and `last_fail_at` in the vault (zeroized on success). Add time-based backoff (exponential, capped at 30 minutes).
- **H-6 Share temppass HMAC vs RSA:** this is tracked under bd-1du.5 and is the main blocker for C-client share interop. Either complete the RSA-4096 signing path, or explicitly flip the matrix row to `Partial` with "symmetric-signature-only; C clients cannot verify."

### P2-3. Transport (§6-opus H-1..H-5)

- **H-1:** `pub use_tls: bool` — make `TransportConfig::use_tls` private; expose only `TransportConfig::production()` / `TransportConfig::dev_plaintext()` constructors; bootstrap assembles via constructor.
- **H-2 TLS pinning:** call `ClientConfig::builder()` with `with_protocol_versions(&[&TLS13, &TLS12])`, add ALPN `h2`/`http/1.1`, keep webpki revocation off (no CRL in webpki).
- **H-3 duplicated TLS config:** single `pcloud-proto::tls::shared_config() -> Arc<ClientConfig>` used by both `transport.rs` and `http_download.rs`.
- **H-4 Retry-After ignored:** thread the parsed `Retry-After` into `RetryPolicy::next_wait` via an override channel.
- **H-5 HTTP download no total timeout:** add `total_request_timeout: Duration` to `http_download.rs`, enforce via `tokio::time::timeout` around the entire read loop.

### P2-3b. CLI/SDK parity gaps (§8-opus H1, H2, M1-M3)

- **H1 completion missing:** add `from-file` under `upload` and `create-tree-link-from-paths` at top level in `crates/pcloud-cli/src/completion.rs:209-221` (bash/zsh/fish).
- **H2 SDK typed helpers:** add `upload_write_from_file()` and `create_tree_public_link_from_paths()` wrappers on `pcloud-sdk/src/lib.rs` (once row 93 re-wired or as stubs returning `not-yet-wired`).
- **M1 SecretPrompt misuse:** switch `app.rs:2763,2770` to plain prompt helper; reserve `SecretPrompt` for credentials only.
- **M2 empty-paths guard:** reject `paths.is_empty()` with `invalid_input(...)` in `Command::CreateTreeLinkFromPaths` before IPC.
- **M3 client-side path validation:** stat/canonicalise local path client-side in `Command::UploadFromFile` (or its replacement) before dispatch.

### P2-4. Security (§2-opus H-1, H-2)

- **H-1 runtime password handlers take bare `String`:** change `runtime.rs` signatures at lines 2199, 2260, 2739, 2774, 2909, 2966 to accept `SecretString` by value. Callers already have a `Zeroizing` wrapper; wire the boundary.
- **H-2 pcloud-web bearer is plain `String`:** wrap in `SecretString`; replace `==` with `subtle::ConstantTimeEq::ct_eq`; add zeroize-on-drop for form passwords in `routes.rs:258`.

### P2-4b. FUSE HIGHs (§5-opus H-1..H-5)

- **H-1 PageCache eviction O(n):** replace scan with a proper LRU (linked-hash-map / `lru` crate).
- **H-2 mountpoint validator TOCTOU:** open+fstat the mountpoint once and pass the fd through the validate→mount path.
- **H-3 `allow_other` contradiction:** reconcile policy layer vs FUSE options layer; single authoritative gate in `PolicyValidator`.
- **H-4 Windows double-reclaim:** audit failure paths in `platform/windows.rs` that free the filesystem object twice on mount failure; add RAII guard.
- **H-5 Windows `fsp_get_user_context_global` non-Sync OnceLock:** replace with `OnceLock<Mutex<...>>` or `parking_lot::Mutex`.

### P2-5. IPC/Daemon (§7-opus H-1..H-4)

- **H-1 privileged logging omits new variants:** add both to `is_privileged_request()` and to the audit event tag set.
- **H-2 sd_notify missing STOPPING/RELOADING:** emit `STOPPING=1` on drain begin, `RELOADING=1` on SIGHUP handler. Surface send errors through `log::warn!`.
- **H-3 daemon has no HTTP health endpoint:** add a minimal `/livez` + `/readyz` on a loopback TCP port (configurable, disabled by default). Consumers: Kubernetes probes.
- **H-4 `accept_and_spawn` unused in production:** document that `RuntimeShell: !Send` blocks migration; add a tracking note.

### P2-6. Sync engine HIGH items (§4-opus, §4-sonnet consensus)

- Planner silent-drop at cap (§4-opus H): add dead-letter persistence in `store_kv`; `RuntimeShell::evict_overflow` drains and replays on next cycle.
- `replace_queue` clobbers cross-root work (§4-opus H / scheduler.rs:80-87): scope replacement to `(sync_root_id, work_kind)` tuple.
- `next_batch` fairness (§4 consensus): swap call site `lib.rs:487` from `next_batch` to `next_batch_fair`.
- Pause state in-memory despite doc claim (§4-opus H): persist `pause_state` in `sync_root_state` table; load on bootstrap.
- `StallDetector` dead code (§4 consensus): wire into `sync_loop_runtime.rs::drive_cycle` after transfer work dispatch.
- `coalesce_window_ms` unused (§4 consensus): either honor in `FsEventIngestor` or delete the field.
- Audit persistence failure swallowed (§4-opus): surface via `CycleResult::audit_persist_error` and fail the cycle if writes fail.

### P2-7. Deployment/packaging

- **systemd `NotifyAccess=main` missing (§11-12-sonnet H-2; Opus-validatable):** with `DynamicUser=yes` and `PrivateUsers=yes`, default `NotifyAccess=none` drops all sd_notify datagrams silently. Add `NotifyAccess=main`. **This is high-impact operational but Sonnet-unique — spawn Opus validator before editing.**
- **FreeBSD CI `continue-on-error: true` (§11-12-sonnet H-1):** remove the flag or downgrade docs to Tier-3 best-effort.
- **API-REFERENCE.md stale (§11-12-opus H-1):** regenerate from the CSV; add a CI job that diffs the doc against the matrix.
- **Prominent enterprise-surface README shield (§11-12-opus H-2):** remove or demote to a per-crate status table.
- **Reproducible-build undefended (§11-12-opus H-3):** add a CI job that builds on two runners and compares SHA-256.
- **No SBOM/cosign (§11-12-opus H-4):** `cargo-auditable` + `syft` for SBOM; cosign for release signing.
- **Packaging MEDIUMs (§11-12-opus MED-1..MED-9):**
  - Ship commented `packaging/systemd/override.conf.example` that broadens `IPAddressAllow` to pCloud API ranges.
  - Fix `pcloudd.socket:3` Documentation URL to `github.com/ezechiel203/pcloud-rs`.
  - Resolve macOS plist path vs signing/notarize targets; link `entitlements.plist` with hardened-runtime entitlement.
  - `nfpm.yaml`: parametrize version from git tag; add arm64 target; wire `.deb` build into CI.
  - Reconcile `pcloudd.rc` `daemon_user` vs `pcloudd_user` (pick `pcloudd`).
  - WiX `pcloud-rs.wxs`: mint frozen `UpgradeCode` GUID; resolve signing cert TODOs (or document deferred-to-release).
  - Add "scaffold / not live" banner to each `docs/enterprise/*` file matching README disclaimer.
  - `fuzz.yml`: persist corpus via `actions/cache` keyed on `fuzz/corpus/*`.
  - Align README "Tier-1" label for FreeBSD with Tier-3 reality.
  - Add mdbook build job to `ci.yml`.

### P2-8. Quality/testing (§9-10 consensus HIGH)

- FreeBSD absent from CI (also in §11-12): add a cirrus-ci or `cross` matrix entry.
- Live-e2e missing for rows 93 (once re-wired), 149, `change_crypto_pass`, backup flows: add tests under `crates/pcloud-live-e2e/tests/` gated on `PCLOUD_LIVE_E2E=1`.
- **§9-10-opus C-09.1:** sweep `.expect(...)` in IPC server/transport paths; replace with graceful error propagation.
- **§9-10-opus H-09.2:** sweep 105 reachable `panic!`/`unreachable!` in production trees; convert to `Err(...)` or document as invariant-enforced.

---

## Priority 3 — MEDIUM (Opus-confirmed, batched)

- Sync engine MEDIUM bucket (§4-opus M-1..M-8): `NewestWins` stub, `RenameBoth` stub, `remote_file_id` dropped on conflict resolve, bandwidth limiter absent, no durable plan queue, `evict_sync_root` leaks `FsWatcher`, `walk_local_tree` symlink/cycle guard.
- FUSE MEDIUM bucket (§5-opus M-1..M-7): `drop(joiner)` detach UAF on macOS, macOS ships with always-erroring mount, write-path flush trigger loss on error, staging dir uid check, `f64` attr timeouts, `OsStr::as_encoded_bytes` non-UTF-8.
- Transport MEDIUM bucket (§6-opus M-1..M-6): happy-eyeballs, API-server allowlist, observability TODOs.
- Crypto MEDIUM (§3-opus M-1..M-5): `getrandom` panic, per-sector `WrappedDek` clone, rotation doesn't re-wrap KMS blobs, `TemppassError` Display variants leak in logs (M-4), replace hand-rolled base64 with `base64` crate (M-5).
- IPC MEDIUM (§7-opus M-1..M-5): privileged log uses daemon uid not peer uid, `retry_after_for` consumes a token, connection cap is process-global, read timeout applied only post-auth (M-4), add dispatch proptest (M-5).
- Transport MEDIUM (§6-opus M-4..M-6): `TransportError::Io` collapses permanent→transient; `parse_retry_after` whitespace handling; retry budget token consumed on `Retry` only not on `RetryAfter`.
- Sync engine MEDIUM (§4-opus M-8): `EngineShell` Clone/PartialEq/Eq derivation over non-trivial state.
- Security MEDIUM (§2-opus M-1..M-4): `id_token` plumbed as plain strings, `RedactedString` lacks ZeroizeOnDrop, no landlock/seccomp.

---

---

## Priority 4 — LOW (Opus-confirmed)

### §1 Parity
- **§1-opus L-1** — Audit log embeds raw `local_path`, leaking filesystem paths on multi-tenant hosts — `crates/pcloud-daemon/src/runtime.rs:2561-2568` — Redact `local_path` to filename-only in audit format strings. (Also tracked under P2-0b.)

### §2 Security
- **§2-opus L-1** — `WebConfig::default` calls `getrandom` with `.expect()` that panics on RNG failure — `crates/pcloud-web/src/lib.rs:124` — Return a typed construction error instead of panicking.
- **§2-opus L-2** — Public example prints `Debug` of a `SecretString`, inviting copy/paste into non-wrapper paths — `crates/pcloud-secret/examples/roundtrip.rs:24` — Add cautionary comment; move demo into integration tests.
- **§2-opus L-3** — `store_token` on Windows does not translate 0700/0600 to an NTFS DACL when `PCLOUD_VAULT=file` is forced — `crates/pcloud-daemon/src/vault/file.rs:143-146` — Refuse `VaultBackend::File` on Windows or apply a DACL equivalent to the DPAPI path.
- **§2-opus L-4** — `TEST_TOKEN` const may be grepped alongside real env names — `crates/pcloud-mockserver/src/lib.rs:84` — Gate behind `#[cfg(test)]` or label as test-only.
- **§2-opus L-5** — `peer_sid` stored per connection without scrubbing on Windows IPC — `crates/pcloud-ipc/src/platform/windows.rs:365` — Document rationale; keep as-is.

### §3 Crypto
- **§3-opus L-1** — Filename HMAC determinism leaks equal plaintext across folders — `crates/pcloud-crypto/src/metadata.rs:80-84` — Surface the trade-off in deployment docs.
- **§3-opus L-2** — `cache_ttl_secs: 300` default is dead policy state with no enforcement — `crates/pcloud-crypto/src/keys.rs:93` — Wire an auto-stop timer on `active_key_material` or remove the field.
- **§3-opus L-3** — Broad `#![allow(clippy::pedantic)]` on a crypto crate — `crates/pcloud-crypto/src/lib.rs:42` — Replace with narrowly-scoped `allow`s per call-site.
- **§3-opus L-4** — `FILENAME_LABEL` / `file-key/v1` labels lack a profile-version epoch — Introduce a top-level `ProfileVersion` constant and serialized discriminant for auditable migrations.

### §4 Sync engine
- **§4-opus L-1** — Multi-root overflow logs a misleading first-candidate `sync_id` — `crates/pcloud-engine/src/planner.rs:123` — Log all affected `sync_id`s or clarify the message.
- **§4-opus L-2** — `ConflictResolver::RenameBoth` behaves identically to `ManualReview` — `conflict_resolver.rs:185-195` — Implement distinct rename-both semantics or collapse the variant.
- **§4-opus L-3** — `execute_downloads` reads entire file into memory — `sync_loop_runtime.rs:370-378` — Chunk `download_bytes` to avoid OOM on large files.
- **§4-opus L-4** — `read_upload_payload` reads whole file via `read_staged_path(0, usize::MAX)` — `sync_loop_runtime.rs:672-684` — Chunk staged reads to bound memory.
- **§4-opus L-5** — `validate_relative_path` triplicated across `fs_events.rs`, `diff_poller.rs`, `local_scan.rs` — Extract to a shared helper.
- **§4-opus L-6** — Zero-timeout stall-detector config yields infinite stalling — `stall_detector.rs:117-121` — Clamp to a reasonable minimum on construction.
- **§4-opus L-7** — `ingest_candidates_filtered` always calls `replace_queue` with destructive semantics not reflected in helper names — Rename helpers or document the replace semantics prominently.
- **§4-opus L-8** — SQLite opens with `synchronous=NORMAL` raising durability concern for cursor writes — `sync_loop_runtime.rs:146` — Use `FULL` specifically for cursor writes.

### §5 FUSE
- **§5-opus L-1** — `MountHandle::Drop` silently swallows unmount errors — `crates/pcloud-fs/src/mount_service.rs:523-544` — Expose a `last_drop_error` atomic or error channel for operator detection.
- **§5-opus L-2** — Validator does not consult `/etc/fuse.conf` when `allow_other` is set, yielding opaque mount failures — `crates/pcloud-fs/src/mount.rs:40-50` — Pre-check `user_allow_other` and surface a clear remediation error.
- **§5-opus L-3** — `PageCache::put` silently drops oversized pages without a metric — `crates/pcloud-fs/src/page_cache.rs:276-280` — Add `bytes_rejected_oversized` counter.
- **§5-opus L-4** — Global `ACTIVE_MOUNTS` registry is unbounded and unbalanced on some failure paths — `crates/pcloud-fs/src/platform/linux.rs:607-611` — Use a canonical-path `BTreeSet` with debug assertions on register/unregister balance.
- **§5-opus L-5** — Linux FUSE write test gated inconsistently with CLAUDE.md env-var naming — `crates/pcloud-fs/src/mount_service.rs:635-637` — Harmonise on a single documented gate variable.
- **§5-opus L-6** — BSD platform returns generic `UnsupportedPlatform` despite tier-3 claim — `crates/pcloud-fs/src/platform/bsd.rs` — Return a specific error with a `fusefs-libs` + `vfs.usermount=1` remediation hint.

### §6 Transport
- **§6-opus L-1** — `is_retryable_io` retries on `BrokenPipe`/`ConnectionReset` inside the deadline loop — `crates/pcloud-transport/src/transport.rs:504-514` — Tighten inner loop to `Interrupted|WouldBlock` only.
- **§6-opus L-2** — `ResponseTooLarge` limit is hard-coded — `transport.rs:408` — Expose via `TransportConfig`.
- **§6-opus L-3** — `MAX_RESPONSE_BYTES = 64 MiB` allocated eagerly from server-advertised `frame_len` — `transport.rs:415` — Stream-parse or use a bounded pool instead of `vec![0u8; frame_len]`.
- **§6-opus L-4** — `fetch_download_resumable` never honours advertised `retry_after` — `crates/pcloud-transport/src/http_download.rs:172-181,461` — Sleep `retry_after()` before retry in the resumable caller.
- **§6-opus L-5** — Every `execute` opens a new TCP+TLS session — `transport.rs:305` — Document the no-keep-alive policy or add pooling.
- **§6-opus L-6** — `default_classifier` marks `InvalidInput` as transient — `crates/pcloud-transport/src/resilient_transport.rs:119` — Invert to "permanent unless explicitly listed".

### §7 IPC/daemon
- **§7-opus L-1** — `backend_label` catch-all silently buckets future `Request::*` variants as "other" — `crates/pcloud-daemon/src/dispatch.rs:205-210` — Add a `log::warn!` in the wildcard arm to surface observability drift.
- **§7-opus L-2** — `write_response` swallows `BrokenPipe`/`ConnectionReset` via `let _ =` — `crates/pcloud-ipc/src/transport.rs:376` — Emit trace-level log on write failure.
- **§7-opus L-3** — `is_privileged_request` omits `LostPassword` and `VerifyEmailRestricted` from audit-trail classification — `crates/pcloud-daemon/src/serve.rs:68-84` — Add both variants to the privileged set.
- **§7-opus L-4** — No explicit `MAINPID=` notification for future fork-after-bind embedders — `crates/pcloud-daemon/src/serve.rs:384` — Add a docstring note anticipating the fork model.

### §8 CLI/SDK
- **§8-opus L-1** — Help text missing `from-file` and `create-tree-link-from-paths` entries — `crates/pcloud-cli/src/app.rs:~105-432` — Add a one-line description per new subcommand.
- **§8-opus L-2** — SDK public response structs lack `#[non_exhaustive]` — `crates/pcloud-sdk/src/lib.rs` — Mark `StatResult`, `FolderEntry`, `PromoResult`, `AuthenticatedUser` as `#[non_exhaustive]`.
- **§8-opus L-3** — No SDK example for `upload_write_from_file` or tree-link-from-paths — `crates/pcloud-sdk/examples/` — Add worked examples for both flows.

### §9-10 Quality/testing
- **§9-opus L-09.1** — `info!`-level "token refreshed successfully" spam on long-running daemon — `crates/pcloud-daemon/src/serve.rs:410` — Demote to `debug!` or rate-limit.
- **§10-opus L-10.1** — No bench for auth vault I/O or FUSE writeback latency — Add two bench targets.
- **§10-opus L-10.2** — Fuzz corpus committed to git may go stale — `crates/pcloud-proto/fuzz/corpus/` — Confirm refresh cadence or move corpus out of the tree.

### §11-12 Deployment/docs
- **§11-opus L-1** — `pcloudd.service` has zero `Environment=` lines despite env-only config — `packaging/systemd/pcloudd.service:24` — Add commented `LoadCredentialEncrypted` stanza and a `PCLOUD_ROOT` default.
- **§11-opus L-2** — `postinst` creates the `fuse` group silently — `packaging/debian/postinst:14-16` — Log the action in the output message at `:18-20`.
- **§12-opus L-3** — `SUMMARY.md` links `../../parity/integrity-sweeper.md` outside `src/` — `docs/book/src/SUMMARY.md:46` — Repair the path and add mdbook build to CI.
- **§12-opus L-4** — No mdbook build job in CI despite required "every chapter builds" gate — `ci.yml` — Add an mdbook-build job.
- **§12-opus L-5** — `CHANGELOG.md` semver discipline unverified against version pinning — `nfpm.yaml:13` — Confirm semver alignment before first release.

---

## Needs Opus Validation (Sonnet-only findings; do NOT act until validated)

Park these until a targeted Opus validator confirms. If confirmed, promote to the corresponding priority bucket.

1. **§1-sonnet H-01, H-02:** `psync_tfa_has_devices` (row 26) and `psync_tfa_type` (row 27) have zero implementing code. Matrix impact: 156/2 → 154/4 if confirmed.
2. **§11-12-sonnet H-2:** systemd `NotifyAccess=main` missing with `DynamicUser=yes`. **High operational impact if true** — validate first.
3. **§3-sonnet M-1:** `encrypt_filename` skips NFC normalization. Opus §3 H-4 covers the same surface; treat as validated.
4. **§8-sonnet M-3:** `EmbeddedDaemon::dispatch` leaks internal `pcloud_ipc::Request`/`Response` types without re-export, creating hidden semver coupling.
5. **§9-10-sonnet H-09.2:** `path_validation.rs:160,173` has `.unwrap()` on `to_str()` for IPC-client paths; non-UTF-8 panics the handler thread.
6. **§9-10-sonnet H-09.3:** `transfer_bridge.rs:250` `chunk_size.expect()` on a config-hot-reload-invalidatable invariant.

**Proposed validator agent prompt:** "Verify each of these 6 Sonnet claims by opening the cited file at the cited line range. Report confirmed/refuted/partial with quoted evidence. 300 words max."

---

## Execution Order

| Wave | Priority | Work | Est. Agents |
|------|----------|------|-------------|
| 1 | P0-1 | Revert row 93 wiring; fix rate-limit bucket; STATUS.md | 1 |
| 1 | — | Validate Sonnet-only claims (6 items) | 1 Opus validator |
| 2 | P1-1..P1-6 | CRITICAL code defects (sync engine, FUSE, macOS vtable, inode, mutex poison, crypto KAT) | 6 parallel agents |
| 3 | P2-1..P2-8 | HIGH severity (parity docs, crypto, transport, security, IPC, sync engine HIGH, packaging, quality) | 8 parallel agents |
| 4 | P3 | MEDIUM sweep | 3-4 parallel agents batched by crate |

## Gate criteria

- After Wave 1: STATUS.md headline honest (157/1/0/28 or 155/3/0/28 if P0-2 confirms).
- After Wave 2: no CRITICAL open; `cargo test --workspace` green.
- After Wave 3: no HIGH open from Opus audit; CI green on Linux + macOS + Windows + FreeBSD (T2 or T1).
- After Wave 4: audit 05 re-run with 10+10 agents shows no regression.

bd-1du.10 gate stays open until all of the above plus cross-platform mount hardware verification + human reviewer sign-off.
